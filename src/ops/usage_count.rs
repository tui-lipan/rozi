//! The anonymous weekly usage count.
//!
//! Once per ISO week, one client for each user account on a computer posts a small report to the
//! usage counter: the release, channel, OS, architecture, install channel, and a coarse
//! install-age bucket. That is the unit counted, an *install*: the state lives in the user's state
//! directory, so two accounts on one computer are two installs. Nothing in the report identifies
//! the install. It carries no install ID, only a random token made fresh for each week's report,
//! which lets the counter recognise a resent report, and the counter keeps per-week aggregate
//! counters, never the request or its address.
//!
//! The rules that make the number trustworthy, and the user's consent meaningful, all live here:
//!
//! - An install is counted at most once a week. A report that got no answer is kept and resent
//!   with the same token, and the counter counts each token once, so a lost answer neither loses
//!   nor doubles the count. The counter's answer names the week it counted, and that week is what
//!   gets marked, so a report sent just before midnight on Sunday is not counted again on Monday.
//!   A report the counter refuses is not retried.
//! - Nothing is sent before the user has been told. A check that finds no record of the notice
//!   claims it and asks the client to show it; only the client, once it has raised the notice,
//!   records it as shown. A claim nobody confirmed lapses, so a client that exits first does not
//!   leave the notice unshown. The first report goes out on a later day than the notice, which
//!   leaves time to opt out.
//! - `[updates] count_usage = false`, `[updates] check = false`, `DO_NOT_TRACK`, a CI environment,
//!   and a debug build each keep the install out of the count.
//!
//! The wire format is a public contract with the counter in `tools/usage-counter/`. Add fields
//! rather than change them, and bump [`SCHEMA_VERSION`] only for a change the counter must
//! recognise.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use tui_lipan::prelude::CommandLink;
use ureq::tls::{RootCerts, TlsConfig};

use crate::Msg;
use crate::platform::install_source::InstallSource;

/// Where reports go. Owned by the tui-lipan project, so other tui-lipan apps can share it.
pub(crate) const ENDPOINT: &str = "https://usage.tui-lipan.dev/v1/count";
/// The user docs page that says what is sent and how to opt out.
pub(crate) const DOCS_URL: &str = concat!(env!("CARGO_PKG_HOMEPAGE"), "/usage-count");

const SCHEMA_VERSION: u32 = 1;
const APP: &str = "rozi";
const DIR_NAME: &str = "usage-count";
/// The date the notice was shown, written by the client that showed it.
const NOTICE_FILE: &str = "notice";
/// A check's claim to show the notice, until a client confirms it was shown.
const NOTICE_CLAIM_FILE: &str = "notice-claim";
/// The token of a report sent without an answer yet.
const PENDING_FILE: &str = "pending";
const FIRST_COUNTED_FILE: &str = "first-counted";
const WEEK_PREFIX: &str = "week-";
const DATE_FORMAT: &str = "%Y-%m-%d";
/// How long an unconfirmed notice claim holds off other clients. Long enough for the claiming
/// client to raise the toast, short enough that one that exited first leaves the notice to the
/// next launch.
const NOTICE_CLAIM_TTL: Duration = Duration::from_secs(10 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_ANSWER_BYTES: u64 = 4 * 1024;
const NIGHTLY_COMMIT: Option<&str> = option_env!("ROZI_NIGHTLY_COMMIT");

/// One week's report. Every value but the token comes from a short fixed vocabulary, and the token
/// is random, so the report cannot carry anything about the install beyond what the field names say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Report {
    schema: u32,
    app: &'static str,
    version: &'static str,
    channel: &'static str,
    os: &'static str,
    arch: &'static str,
    install: &'static str,
    age: u8,
    token: String,
}

/// What a check did, for the caller and for tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// First run for this install: nothing was sent, and the client should show the notice.
    NoticeDue,
    /// The notice is being shown, or went up today; the first report waits for a later day.
    WaitingAfterNotice,
    /// This install was already counted this week, possibly by another client.
    AlreadyCounted,
    /// The report was accepted.
    Counted,
    /// The counter refused the report. Retrying this week would only repeat the refusal.
    Rejected,
    /// The report did not get through, or its answer was lost; a later check resends it.
    Failed,
}

/// How a post ended, as far as retrying is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Delivery {
    /// The counter has the report. `counted` is false when it had counted this token before.
    Accepted { week: String, counted: bool },
    /// A 4xx answer: the counter understood the request and will not take it.
    Rejected,
    /// No answer, an unreadable answer, or a server error. The report may or may not have been
    /// counted; resending its token settles that.
    Failed,
}

/// The network edge, replaced in tests.
pub(crate) trait Transport {
    fn post(&self, url: &str, body: &[u8]) -> Delivery;
}

/// Why this install might be excluded regardless of configuration.
#[derive(Debug, Clone, Copy)]
struct Exclusions {
    do_not_track: bool,
    ci: bool,
    debug_build: bool,
}

impl Exclusions {
    fn from_process() -> Self {
        Self {
            do_not_track: truthy_env(std::env::var_os("DO_NOT_TRACK")),
            ci: std::env::var_os("CI").is_some_and(|value| !value.is_empty()),
            debug_build: cfg!(debug_assertions),
        }
    }

    fn any(self) -> bool {
        self.do_not_track || self.ci || self.debug_build
    }
}

/// `DO_NOT_TRACK=1` is the convention; any non-empty value other than `0` or `false` counts, since
/// someone who set the variable at all meant it.
fn truthy_env(value: Option<std::ffi::OsString>) -> bool {
    value.is_some_and(|value| {
        let value = value.to_string_lossy();
        let value = value.trim();
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

/// This user's usage-count state, when the platform names a state directory.
fn state_dir() -> Option<PathBuf> {
    let env = crate::platform::paths::PlatformEnv::from_process();
    crate::platform::paths::state_dir_if_available(&env).map(|dir| dir.join(DIR_NAME))
}

/// Run one check from a client's update loop. `enabled` is the client's own gate: release checks
/// allowed, `[updates] check` and `[updates] count_usage` all on.
///
/// Blocks on the network, so the caller is a worker thread.
pub(crate) fn report_if_due(link: &CommandLink<Msg>, enabled: bool) {
    if !enabled || Exclusions::from_process().any() {
        return;
    }
    let Some(dir) = state_dir() else {
        return;
    };
    let template = Report::current(crate::platform::install_source::detect_current());
    if run(&dir, Utc::now(), &template, &HttpsTransport::new()) == Outcome::NoticeDue {
        link.send(Msg::UsageCountNotice);
    }
}

/// Record that this client has shown the notice. Called by the client after it raises the notice,
/// never before: until then, the notice counts as not shown.
pub(crate) fn notice_shown() {
    if let Some(dir) = state_dir() {
        record_notice_shown(&dir, Utc::now().date_naive());
    }
}

#[cfg(test)]
pub(crate) fn notice_recorded_for_test() -> bool {
    state_dir().is_some_and(|dir| read_date(&dir.join(NOTICE_FILE)).is_some())
}

impl Report {
    /// This build's report. [`run`] fills in the age and the token.
    fn current(install: InstallSource) -> Self {
        Self {
            schema: SCHEMA_VERSION,
            app: APP,
            version: env!("CARGO_PKG_VERSION"),
            channel: if NIGHTLY_COMMIT.is_some() {
                "nightly"
            } else {
                "release"
            },
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            install: install_name(install),
            age: 1,
            token: String::new(),
        }
    }
}

/// The wire name of each install channel. Exhaustive on purpose: a new channel has to choose its
/// name here, and the counter has to learn it, before it can be counted as itself.
fn install_name(source: InstallSource) -> &'static str {
    match source {
        InstallSource::Managed => "managed",
        InstallSource::Cargo => "cargo",
        InstallSource::Mise => "mise",
        InstallSource::Homebrew => "homebrew",
        InstallSource::Scoop => "scoop",
        InstallSource::WinGet => "winget",
        InstallSource::SystemPackage => "system",
        InstallSource::Unknown => "unknown",
    }
}

/// Install-age bucket from the date this install was first counted. Coarse on purpose: it tells
/// new installs from returning ones without dating any of them.
///
/// 1: first week, 2: within four weeks, 3: within 24 weeks, 4: older.
fn age_bucket(first_counted: NaiveDate, today: NaiveDate) -> u8 {
    match (today - first_counted).num_days() {
        ..7 => 1,
        7..28 => 2,
        28..168 => 3,
        _ => 4,
    }
}

/// `2026-W44` for any day of that ISO week. Sorts in week order.
fn iso_week(day: NaiveDate) -> String {
    let week = day.iso_week();
    format!("{}-W{:02}", week.year(), week.week())
}

fn is_iso_week(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 8
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && &bytes[4..6] == b"-W"
        && bytes[6..].iter().all(u8::is_ascii_digit)
}

/// 128 random bits as hex. Made fresh for each week's report and never reused for another.
fn new_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("operating-system randomness unavailable");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// One check against the state in `dir`, as of `now`. `template` is this build's report; the age
/// and token are filled in here.
pub(crate) fn run(
    dir: &Path,
    now: DateTime<Utc>,
    template: &Report,
    transport: &impl Transport,
) -> Outcome {
    if crate::platform::fs_security::ensure_private_dir(dir).is_err() {
        return Outcome::Failed;
    }
    let today = now.date_naive();
    match notice_state(dir, now) {
        Notice::Claimed => return Outcome::NoticeDue,
        Notice::Showing => return Outcome::WaitingAfterNotice,
        Notice::Shown(shown) if shown >= today => return Outcome::WaitingAfterNotice,
        Notice::Shown(_) => {}
        Notice::Unavailable => return Outcome::Failed,
    }

    // A report whose answer never came goes first, under its own token: the counter either counts
    // it now or says it already did, and which week.
    if let Some(token) = read_pending(dir) {
        match send(dir, today, template, token, transport) {
            Outcome::Counted | Outcome::AlreadyCounted | Outcome::Rejected => {}
            other => return other,
        }
    }

    if counted_week(dir).is_some_and(|week| week >= iso_week(today)) {
        return Outcome::AlreadyCounted;
    }
    let token = new_token();
    match crate::platform::persist::create_private_file(&dir.join(PENDING_FILE), false) {
        Ok(_) => {}
        // Another client is sending this week's report right now.
        Err(error) if error.kind() == ErrorKind::AlreadyExists => return Outcome::AlreadyCounted,
        Err(_) => return Outcome::Failed,
    }
    if crate::platform::persist::replace_file(&dir.join(PENDING_FILE), &token).is_err() {
        let _ = fs::remove_file(dir.join(PENDING_FILE));
        return Outcome::Failed;
    }
    send(dir, today, template, token, transport)
}

/// Post `token`'s report and settle the local state from the answer. The pending file stays until
/// the counter answers, so only an answer ends a report.
fn send(
    dir: &Path,
    today: NaiveDate,
    template: &Report,
    token: String,
    transport: &impl Transport,
) -> Outcome {
    let first_counted = read_date(&dir.join(FIRST_COUNTED_FILE));
    let report = Report {
        age: age_bucket(first_counted.unwrap_or(today), today),
        token,
        ..template.clone()
    };
    let Ok(body) = serde_json::to_vec(&report) else {
        return Outcome::Failed;
    };
    match transport.post(ENDPOINT, &body) {
        Delivery::Accepted { week, counted } => {
            if counted && first_counted.is_none() {
                let _ = write_date(&dir.join(FIRST_COUNTED_FILE), today);
            }
            mark_counted(dir, &week);
            let _ = fs::remove_file(dir.join(PENDING_FILE));
            if counted {
                Outcome::Counted
            } else {
                Outcome::AlreadyCounted
            }
        }
        Delivery::Rejected => {
            mark_counted(dir, &iso_week(today));
            let _ = fs::remove_file(dir.join(PENDING_FILE));
            Outcome::Rejected
        }
        Delivery::Failed => Outcome::Failed,
    }
}

fn read_pending(dir: &Path) -> Option<String> {
    let path = dir.join(PENDING_FILE);
    let text = fs::read_to_string(&path).ok()?;
    let token = text.trim();
    if token.len() == 32
        && token
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Some(token.to_string());
    }
    // An empty file is a claim another client is about to fill; anything else is damage.
    if !token.is_empty() {
        let _ = fs::remove_file(path);
    }
    None
}

/// The latest week this install is marked as counted in.
fn counted_week(dir: &Path) -> Option<String> {
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| name.strip_prefix(WEEK_PREFIX).map(str::to_string))
        .filter(|week| is_iso_week(week))
        .max()
}

/// Mark `week` as counted and drop older marks, so the directory holds one at most. A week the
/// counter named in an unexpected shape is not marked; the next check settles it.
fn mark_counted(dir: &Path, week: &str) {
    if !is_iso_week(week) {
        return;
    }
    let marker = format!("{WEEK_PREFIX}{week}");
    match crate::platform::persist::create_private_file(&dir.join(&marker), false) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(_) => return,
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(WEEK_PREFIX) && name < marker.as_str() {
            let _ = fs::remove_file(entry.path());
        }
    }
}

enum Notice {
    /// This check claimed the notice, so its client shows it.
    Claimed,
    /// Another client claimed it moments ago and is showing it.
    Showing,
    Shown(NaiveDate),
    Unavailable,
}

/// Whether the notice was shown and when, claiming it if it was not and nobody holds a live claim.
fn notice_state(dir: &Path, now: DateTime<Utc>) -> Notice {
    let shown_path = dir.join(NOTICE_FILE);
    match fs::read_to_string(&shown_path) {
        Ok(text) => {
            return match NaiveDate::parse_from_str(text.trim(), DATE_FORMAT) {
                Ok(shown) => Notice::Shown(shown),
                // Only a client that showed the notice writes this file, so a damaged one still
                // means it was shown. Repair it as today, which holds the first report back a day.
                Err(_) => {
                    record_notice_shown(dir, now.date_naive());
                    Notice::Showing
                }
            };
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(_) => return Notice::Unavailable,
    }

    let claim = dir.join(NOTICE_CLAIM_FILE);
    match crate::platform::persist::create_private_file(&claim, false) {
        Ok(_) => Notice::Claimed,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            if claim_is_live(&claim, now) {
                return Notice::Showing;
            }
            // The client that claimed it never confirmed. Showing the notice twice is better than
            // not at all, so this check takes the claim over.
            match crate::platform::persist::create_private_file(&claim, true) {
                Ok(_) => Notice::Claimed,
                Err(_) => Notice::Unavailable,
            }
        }
        Err(_) => Notice::Unavailable,
    }
}

fn claim_is_live(claim: &Path, now: DateTime<Utc>) -> bool {
    let Ok(claimed) = fs::metadata(claim).and_then(|metadata| metadata.modified()) else {
        return false;
    };
    let now = SystemTime::from(now);
    // A claim from the future is a clock that moved; it holds no longer than one made just now.
    now.duration_since(claimed)
        .map_or(true, |age| age < NOTICE_CLAIM_TTL)
        && claimed
            .duration_since(now)
            .map_or(true, |ahead| ahead < NOTICE_CLAIM_TTL)
}

fn record_notice_shown(dir: &Path, today: NaiveDate) {
    if crate::platform::fs_security::ensure_private_dir(dir).is_err() {
        return;
    }
    if write_date(&dir.join(NOTICE_FILE), today).is_ok() {
        let _ = fs::remove_file(dir.join(NOTICE_CLAIM_FILE));
    }
}

fn read_date(path: &Path) -> Option<NaiveDate> {
    let text = fs::read_to_string(path).ok()?;
    NaiveDate::parse_from_str(text.trim(), DATE_FORMAT).ok()
}

fn write_date(path: &Path, date: NaiveDate) -> std::io::Result<()> {
    crate::platform::persist::replace_file(path, date.format(DATE_FORMAT).to_string())
}

struct HttpsTransport {
    agent: ureq::Agent,
}

impl HttpsTransport {
    fn new() -> Self {
        let config = ureq::Agent::config_builder()
            // The same trust store the release check uses, so a machine behind an inspecting
            // proxy reaches both or neither.
            .tls_config(
                TlsConfig::builder()
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .https_only(true)
            .max_redirects(0)
            .timeout_global(Some(REQUEST_TIMEOUT))
            .http_status_as_error(false)
            .user_agent(concat!("rozi/", env!("CARGO_PKG_VERSION")))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

#[derive(Deserialize)]
struct Answer {
    week: String,
    counted: bool,
}

impl Transport for HttpsTransport {
    fn post(&self, url: &str, body: &[u8]) -> Delivery {
        let Ok(mut response) = self
            .agent
            .post(url)
            .header("content-type", "application/json")
            .send(body)
        else {
            return Delivery::Failed;
        };
        match response.status().as_u16() {
            200..=299 => {}
            // A timeout or a rate limit says nothing about the report itself.
            408 | 429 => return Delivery::Failed,
            400..=499 => return Delivery::Rejected,
            _ => return Delivery::Failed,
        }
        response
            .body_mut()
            .with_config()
            .limit(MAX_ANSWER_BYTES)
            .read_to_vec()
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Answer>(&bytes).ok())
            .map_or(Delivery::Failed, |answer| Delivery::Accepted {
                week: answer.week,
                counted: answer.counted,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// The counter's behavior at the edge rozi sees it: each token counted once, in the week it
    /// first arrived. `lose_answers` drops the answer after the counter took the report, which is
    /// the case a retry must not double.
    struct FakeCounter {
        week: RefCell<String>,
        answer: RefCell<Option<Delivery>>,
        lose_answers: RefCell<bool>,
        tokens: RefCell<HashMap<String, String>>,
        sent: RefCell<Vec<serde_json::Value>>,
    }

    impl FakeCounter {
        fn new() -> Self {
            Self {
                week: RefCell::new(String::new()),
                answer: RefCell::new(None),
                lose_answers: RefCell::new(false),
                tokens: RefCell::new(HashMap::new()),
                sent: RefCell::new(Vec::new()),
            }
        }

        /// Answer every post with `delivery`, without taking the report.
        fn answering(delivery: Delivery) -> Self {
            let counter = Self::new();
            *counter.answer.borrow_mut() = Some(delivery);
            counter
        }

        fn at(&self, week: &str) -> &Self {
            *self.week.borrow_mut() = week.to_string();
            self
        }

        fn sent(&self) -> Vec<serde_json::Value> {
            self.sent.borrow().clone()
        }

        /// How many installs the counter has counted in `week`.
        fn counted_in(&self, week: &str) -> usize {
            self.tokens.borrow().values().filter(|w| *w == week).count()
        }
    }

    impl Transport for FakeCounter {
        fn post(&self, url: &str, body: &[u8]) -> Delivery {
            assert_eq!(url, ENDPOINT);
            let report: serde_json::Value = serde_json::from_slice(body).expect("JSON report");
            self.sent.borrow_mut().push(report.clone());
            if let Some(answer) = self.answer.borrow().clone() {
                return answer;
            }
            let token = report["token"].as_str().unwrap().to_string();
            let mut tokens = self.tokens.borrow_mut();
            let counted = !tokens.contains_key(&token);
            let week = tokens
                .entry(token)
                .or_insert_with(|| self.week.borrow().clone())
                .clone();
            if *self.lose_answers.borrow() {
                return Delivery::Failed;
            }
            Delivery::Accepted { week, counted }
        }
    }

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().to_utc()
    }

    fn template() -> Report {
        Report::current(InstallSource::Managed)
    }

    fn scratch() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(DIR_NAME);
        (root, dir)
    }

    /// An install whose notice went up on `day`, as the client records it.
    fn told_on(day: &str) -> (tempfile::TempDir, PathBuf) {
        let (root, dir) = scratch();
        let counter = FakeCounter::new();
        let now = at(&format!("{day}T09:00:00Z"));
        assert_eq!(run(&dir, now, &template(), &counter), Outcome::NoticeDue);
        record_notice_shown(&dir, now.date_naive());
        (root, dir)
    }

    /// The notice goes up first, and nothing is sent until a later day.
    #[test]
    fn nothing_is_sent_before_a_day_has_passed_since_the_notice() {
        let (_root, dir) = scratch();
        let counter = FakeCounter::new();
        counter.at("2026-W44");

        let morning = at("2026-10-29T09:00:00Z");
        assert_eq!(
            run(&dir, morning, &template(), &counter),
            Outcome::NoticeDue
        );
        record_notice_shown(&dir, morning.date_naive());
        let evening = at("2026-10-29T21:00:00Z");
        assert_eq!(
            run(&dir, evening, &template(), &counter),
            Outcome::WaitingAfterNotice
        );
        assert!(counter.sent().is_empty());

        let tomorrow = at("2026-10-30T09:00:00Z");
        assert_eq!(run(&dir, tomorrow, &template(), &counter), Outcome::Counted);
        assert_eq!(counter.counted_in("2026-W44"), 1);
    }

    /// A client that claimed the notice and exited before showing it leaves nothing recorded as
    /// shown, so a later launch shows it, and nothing is sent in the meantime.
    #[test]
    fn a_notice_never_shown_is_claimed_again_and_nothing_is_sent() {
        let (_root, dir) = scratch();
        let counter = FakeCounter::new();
        let start = at("2026-10-29T09:00:00Z");
        assert_eq!(run(&dir, start, &template(), &counter), Outcome::NoticeDue);
        // The claim's age is its file time; pin it to this test's clock.
        let claim = dir.join(NOTICE_CLAIM_FILE);
        fs::File::options()
            .write(true)
            .open(&claim)
            .unwrap()
            .set_modified(SystemTime::from(start))
            .unwrap();

        // Another client a moment later leaves the notice to the first one.
        let moment = at("2026-10-29T09:01:00Z");
        assert_eq!(
            run(&dir, moment, &template(), &counter),
            Outcome::WaitingAfterNotice
        );

        // The first one never confirmed. Days later, the notice is still due, not skipped.
        let days_later = at("2026-11-03T09:00:00Z");
        assert_eq!(
            run(&dir, days_later, &template(), &counter),
            Outcome::NoticeDue
        );
        assert!(counter.sent().is_empty());
    }

    #[test]
    fn an_install_is_counted_once_per_iso_week() {
        let (_root, dir) = told_on("2026-10-20");
        let counter = FakeCounter::new();

        // 2026-10-26 is a Monday: Monday to Sunday is one week, the next Monday a new one.
        counter.at("2026-W44");
        for time in [
            "2026-10-26T09:00:00Z",
            "2026-10-28T09:00:00Z",
            "2026-11-01T22:00:00Z",
        ] {
            run(&dir, at(time), &template(), &counter);
        }
        assert_eq!(counter.sent().len(), 1);

        counter.at("2026-W45");
        assert_eq!(
            run(&dir, at("2026-11-02T09:00:00Z"), &template(), &counter),
            Outcome::Counted
        );
        assert_eq!(counter.counted_in("2026-W44"), 1);
        assert_eq!(counter.counted_in("2026-W45"), 1);
        assert_eq!(counted_week(&dir).as_deref(), Some("2026-W45"));
        let markers = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(WEEK_PREFIX))
            .count();
        assert_eq!(markers, 1);
    }

    /// The counter took the report but the answer was lost. The resend carries the same token, so
    /// the install is counted once, and no new report is made for the same week.
    #[test]
    fn a_report_whose_answer_was_lost_is_resent_and_counted_once() {
        let (_root, dir) = told_on("2026-10-20");
        let counter = FakeCounter::new();
        counter.at("2026-W44");
        *counter.lose_answers.borrow_mut() = true;
        assert_eq!(
            run(&dir, at("2026-10-26T09:00:00Z"), &template(), &counter),
            Outcome::Failed
        );

        *counter.lose_answers.borrow_mut() = false;
        assert_eq!(
            run(&dir, at("2026-10-26T15:00:00Z"), &template(), &counter),
            Outcome::AlreadyCounted
        );
        let sent = counter.sent();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0]["token"], sent[1]["token"]);
        assert_eq!(counter.counted_in("2026-W44"), 1);
        assert!(!dir.join(PENDING_FILE).exists());
    }

    /// Sent late on Sunday by the client's clock, the report lands in Monday's week at the counter.
    /// The client marks the counter's week, so Monday does not count it again.
    #[test]
    fn the_week_the_counter_names_is_the_week_marked() {
        let (_root, dir) = told_on("2026-10-20");
        let counter = FakeCounter::new();
        counter.at("2026-W45");
        assert_eq!(
            run(&dir, at("2026-11-01T23:59:59Z"), &template(), &counter),
            Outcome::Counted
        );
        assert_eq!(
            run(&dir, at("2026-11-02T09:00:00Z"), &template(), &counter),
            Outcome::AlreadyCounted
        );
        assert_eq!(counter.counted_in("2026-W45"), 1);
        assert_eq!(counter.sent().len(), 1);
    }

    /// A report still unanswered when the week turns over is settled first. Once the counter says
    /// it counted that one last week, a fresh report counts this week.
    #[test]
    fn last_weeks_unanswered_report_is_settled_before_this_weeks() {
        let (_root, dir) = told_on("2026-10-20");
        let counter = FakeCounter::new();
        counter.at("2026-W44");
        *counter.lose_answers.borrow_mut() = true;
        run(&dir, at("2026-10-30T09:00:00Z"), &template(), &counter);

        *counter.lose_answers.borrow_mut() = false;
        counter.at("2026-W45");
        assert_eq!(
            run(&dir, at("2026-11-03T09:00:00Z"), &template(), &counter),
            Outcome::Counted
        );
        assert_eq!(counter.counted_in("2026-W44"), 1);
        assert_eq!(counter.counted_in("2026-W45"), 1);
        assert_eq!(counter.sent().len(), 3);
    }

    #[test]
    fn a_rejected_report_is_not_retried_that_week() {
        let (_root, dir) = told_on("2026-10-20");
        let refusing = FakeCounter::answering(Delivery::Rejected);
        assert_eq!(
            run(&dir, at("2026-11-02T09:00:00Z"), &template(), &refusing),
            Outcome::Rejected
        );
        assert_eq!(
            run(&dir, at("2026-11-03T09:00:00Z"), &template(), &refusing),
            Outcome::AlreadyCounted
        );
        assert_eq!(refusing.sent().len(), 1);
    }

    /// The age bucket counts from the first accepted report, not from the notice or a failed send.
    #[test]
    fn age_moves_through_its_buckets_from_the_first_accepted_report() {
        let (_root, dir) = told_on("2025-12-31");
        let counter = FakeCounter::new();
        for (time, week) in [
            ("2026-01-06T09:00:00Z", "2026-W02"),
            ("2026-01-13T09:00:00Z", "2026-W03"),
            ("2026-02-10T09:00:00Z", "2026-W07"),
            ("2026-06-23T09:00:00Z", "2026-W26"),
        ] {
            counter.at(week);
            run(&dir, at(time), &template(), &counter);
        }
        let ages: Vec<u64> = counter
            .sent()
            .iter()
            .map(|report| report["age"].as_u64().unwrap())
            .collect();
        assert_eq!(ages, vec![1, 2, 3, 4]);
    }

    /// Each week's report has a token of its own, so tokens cannot link one week to the next.
    #[test]
    fn every_weeks_report_has_a_fresh_token() {
        let (_root, dir) = told_on("2026-10-20");
        let counter = FakeCounter::new();
        counter.at("2026-W44");
        run(&dir, at("2026-10-26T09:00:00Z"), &template(), &counter);
        counter.at("2026-W45");
        run(&dir, at("2026-11-02T09:00:00Z"), &template(), &counter);
        let sent = counter.sent();
        assert_ne!(sent[0]["token"], sent[1]["token"]);
        assert!(!dir.join(PENDING_FILE).exists());
    }

    /// The report is the whole contract with the counter: these fields, and nothing else.
    #[test]
    fn the_report_carries_only_its_fixed_fields() {
        let (_root, dir) = told_on("2026-10-01");
        let counter = FakeCounter::new();
        counter.at("2026-W40");
        run(&dir, at("2026-10-02T09:00:00Z"), &template(), &counter);

        let sent = counter.sent();
        let object = sent[0].as_object().unwrap();
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "age", "app", "arch", "channel", "install", "os", "schema", "token", "version"
            ]
        );
        assert_eq!(object["schema"], 1);
        assert_eq!(object["app"], "rozi");
        assert_eq!(object["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(object["install"], "managed");
        assert_eq!(object["os"], std::env::consts::OS);
        let token = object["token"].as_str().unwrap();
        assert_eq!(token.len(), 32);
        assert!(
            token
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );

        // The counter's tests post this file, so the two sides agree on every field and its type.
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tools/usage-counter/test/client-report.json"
        ))
        .unwrap();
        let fixture = fixture.as_object().unwrap();
        assert_eq!(fixture.len(), object.len());
        for (key, value) in object {
            let expected = &fixture[key];
            assert_eq!(
                std::mem::discriminant(value),
                std::mem::discriminant(expected),
                "{key}: {value} against the counter's {expected}"
            );
        }
        assert_eq!(object["schema"], fixture["schema"]);
        assert_eq!(object["app"], fixture["app"]);
    }

    /// Every install channel's wire name is one the counter lists.
    #[test]
    fn every_install_channel_has_a_name_the_counter_knows() {
        let counter = include_str!("../../tools/usage-counter/src/report.js");
        for source in [
            InstallSource::Managed,
            InstallSource::Cargo,
            InstallSource::Mise,
            InstallSource::Homebrew,
            InstallSource::Scoop,
            InstallSource::WinGet,
            InstallSource::SystemPackage,
            InstallSource::Unknown,
        ] {
            let name = install_name(source);
            assert!(counter.contains(&format!("\"{name}\"")), "{name}");
        }
    }

    #[test]
    fn a_damaged_notice_record_delays_the_first_report_by_a_day() {
        let (_root, dir) = scratch();
        crate::platform::fs_security::ensure_private_dir(&dir).unwrap();
        fs::write(dir.join(NOTICE_FILE), "").unwrap();
        let counter = FakeCounter::new();
        counter.at("2026-W44");

        assert_eq!(
            run(&dir, at("2026-10-29T09:00:00Z"), &template(), &counter),
            Outcome::WaitingAfterNotice
        );
        assert_eq!(
            run(&dir, at("2026-10-30T09:00:00Z"), &template(), &counter),
            Outcome::Counted
        );
    }

    #[test]
    fn iso_weeks_follow_the_counters_format() {
        let day = |text: &str| NaiveDate::parse_from_str(text, DATE_FORMAT).unwrap();
        assert_eq!(iso_week(day("2026-10-26")), "2026-W44");
        assert_eq!(iso_week(day("2027-01-01")), "2026-W53");
        assert_eq!(iso_week(day("2025-12-29")), "2026-W01");
        assert!(is_iso_week("2026-W44"));
        assert!(!is_iso_week("2026-44"));
        assert!(!is_iso_week("../../etc"));
    }

    /// rozi's real HTTPS client against the live counter. The report is invalid on purpose, so the
    /// counter refuses it and stores nothing. Run by hand: `cargo test --lib live_counter -- --ignored`.
    #[test]
    #[ignore = "reaches the live counter"]
    fn live_counter_refuses_an_invalid_report_over_https() {
        let body = serde_json::to_vec(&Report {
            token: "not-a-token".to_string(),
            ..template()
        })
        .unwrap();
        assert_eq!(
            HttpsTransport::new().post(ENDPOINT, &body),
            Delivery::Rejected
        );
    }

    #[test]
    fn do_not_track_accepts_the_usual_spellings() {
        let set = |value: &str| truthy_env(Some(value.into()));
        assert!(set("1"));
        assert!(set("true"));
        assert!(set("yes"));
        assert!(!set("0"));
        assert!(!set("false"));
        assert!(!set(""));
        assert!(!truthy_env(None));
    }
}
