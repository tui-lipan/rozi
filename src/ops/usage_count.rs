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
//!   nor doubles the count. The token is dropped only once everything its answer settles is on
//!   disk. Weeks are the counter's alone: before making a new report the client asks the counter
//!   which week it is, and makes one only once that week is past the week the last report was
//!   counted in. A wrong local clock therefore cannot count an install twice in a week or change
//!   its age. A report the counter refuses is not retried.
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

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use tui_lipan::prelude::CommandLink;
use ureq::tls::{RootCerts, TlsConfig};

use crate::Msg;
use crate::platform::install_source::InstallSource;

/// Where reports go. Owned by the tui-lipan project, so other tui-lipan apps can share it.
pub(crate) const ENDPOINT: &str = "https://usage.tui-lipan.dev/v1/count";
/// The counter's current week, asked before each new report.
const WEEK_ENDPOINT: &str = "https://usage.tui-lipan.dev/v1/week";
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
/// The counter's week of this install's first count.
const FIRST_COUNTED_FILE: &str = "first-counted";
/// The counter's week of the last settled report.
const COUNTED_FILE: &str = "counted";
const DATE_FORMAT: &str = "%Y-%m-%d";
/// How long a claim holds off other clients: an unconfirmed notice claim, or a report claim with no
/// token in it yet. Long enough for the claiming client to finish, short enough that one that
/// exited first leaves the work to the next launch.
const CLAIM_TTL: Duration = Duration::from_secs(10 * 60);
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
    /// The counter's current week, or `None` when it cannot be reached.
    fn current_week(&self) -> Option<String>;
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
    state_dir()
        .and_then(|dir| fs::read_to_string(dir.join(NOTICE_FILE)).ok())
        .is_some_and(|text| NaiveDate::parse_from_str(text.trim(), DATE_FORMAT).is_ok())
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

/// Install-age bucket from the counter's week of this install's first count and its week now.
/// Coarse on purpose: it tells new installs from returning ones without dating any of them. Both
/// weeks come from the counter's clock, so a wrong local clock cannot make an install look new.
///
/// 1: first week, 2: within four weeks, 3: within 24 weeks, 4: older.
fn age_bucket(first_counted: &str, current: &str) -> u8 {
    let (Some(first), Some(current)) = (week_start(first_counted), week_start(current)) else {
        return 1;
    };
    match (current - first).num_weeks() {
        ..1 => 1,
        1..4 => 2,
        4..24 => 3,
        _ => 4,
    }
}

/// The Monday that starts the ISO week `2026-W44`.
fn week_start(week: &str) -> Option<NaiveDate> {
    if !is_iso_week(week) {
        return None;
    }
    let year = week[..4].parse().ok()?;
    let number = week[6..].parse().ok()?;
    NaiveDate::from_isoywd_opt(year, number, chrono::Weekday::Mon)
}

/// `2026-W44` for any day of that ISO week. Sorts in week order.
#[cfg(test)]
fn iso_week(day: NaiveDate) -> String {
    use chrono::Datelike;
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
///
/// `now` decides only when the notice's day has passed. Which week it is comes from the counter.
pub(crate) fn run(
    dir: &Path,
    now: DateTime<Utc>,
    template: &Report,
    transport: &impl Transport,
) -> Outcome {
    if crate::platform::fs_security::ensure_private_dir(dir).is_err() {
        return Outcome::Failed;
    }
    match notice_state(dir, now) {
        Notice::Claimed => return Outcome::NoticeDue,
        Notice::Showing => return Outcome::WaitingAfterNotice,
        Notice::Shown(shown) if shown >= now.date_naive() => return Outcome::WaitingAfterNotice,
        Notice::Shown(_) => {}
        Notice::Unavailable => return Outcome::Failed,
    }

    // Another client is sending a report right now.
    let pending = match read_pending(dir, now) {
        PendingState::Filling => return Outcome::AlreadyCounted,
        PendingState::Report(token) => Some(token),
        PendingState::Absent => None,
    };
    let Some(week) = transport.current_week().filter(|week| is_iso_week(week)) else {
        return Outcome::Failed;
    };

    // A report whose answer never came goes first, under its own token: the counter either counts
    // it now or says it already did, and which week.
    if let Some(token) = pending {
        match send(dir, &week, template, &token, transport) {
            Outcome::Counted | Outcome::AlreadyCounted | Outcome::Rejected => {}
            other => return other,
        }
    }

    // A new report only once the counter's week has moved past the last one counted. The counter
    // counts a report no earlier than the week it just named, so two reports can never land in
    // the same week of its clock, whatever this one says.
    if read_counted(dir).is_some_and(|counted| counted.as_str() >= week.as_str()) {
        return Outcome::AlreadyCounted;
    }
    // Claim the report before making it, so of several clients exactly one does. The claim is
    // empty until its token is written; one left empty by a client that died there goes stale.
    match crate::platform::persist::create_private_file(&dir.join(PENDING_FILE), false) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => return Outcome::AlreadyCounted,
        Err(_) => return Outcome::Failed,
    }
    let token = new_token();
    if crate::platform::persist::replace_file(&dir.join(PENDING_FILE), &token).is_err() {
        let _ = fs::remove_file(dir.join(PENDING_FILE));
        return Outcome::Failed;
    }
    send(dir, &week, template, &token, transport)
}

fn is_token(text: &str) -> bool {
    text.len() == 32
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

enum PendingState {
    /// The token of a report sent without an answer yet.
    Report(String),
    /// A fresh claim another client has not written its token into yet.
    Filling,
    Absent,
}

fn read_pending(dir: &Path, now: DateTime<Utc>) -> PendingState {
    let path = dir.join(PENDING_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return PendingState::Absent,
        // Present but unreadable: leave it, rather than make a second report beside it.
        Err(_) => return PendingState::Filling,
    };
    let token = text.trim();
    if is_token(token) {
        return PendingState::Report(token.to_string());
    }
    if token.is_empty() && claim_is_live(&path, now) {
        return PendingState::Filling;
    }
    // A claim whose client died before writing it, or damage. Either way no token survives in it.
    let _ = fs::remove_file(path);
    PendingState::Absent
}

/// Post `token`'s report and settle the local state from the answer. `week` is the counter's
/// current week. The pending file is removed only after everything the answer settles is on
/// disk, so a failed write leaves the token to be resent, which the counter answers without
/// counting it again.
fn send(
    dir: &Path,
    week: &str,
    template: &Report,
    token: &str,
    transport: &impl Transport,
) -> Outcome {
    let first_counted = read_week(&dir.join(FIRST_COUNTED_FILE));
    let report = Report {
        age: first_counted
            .as_deref()
            .map_or(1, |first| age_bucket(first, week)),
        token: token.to_string(),
        ..template.clone()
    };
    let Ok(body) = serde_json::to_vec(&report) else {
        return Outcome::Failed;
    };
    let (outcome, counted_in) = match transport.post(ENDPOINT, &body) {
        Delivery::Accepted {
            week: counted_in,
            counted,
        } => {
            let counted_in = if is_iso_week(&counted_in) {
                counted_in
            } else {
                week.to_string()
            };
            // Any acceptance of a token this install made means the install has been counted,
            // including an answer that it was counted before: that earlier count was this one,
            // with its answer lost.
            if first_counted.is_none()
                && write_week(&dir.join(FIRST_COUNTED_FILE), &counted_in).is_err()
            {
                return Outcome::Failed;
            }
            let outcome = if counted {
                Outcome::Counted
            } else {
                Outcome::AlreadyCounted
            };
            (outcome, counted_in)
        }
        Delivery::Rejected => (Outcome::Rejected, week.to_string()),
        Delivery::Failed => return Outcome::Failed,
    };
    // Never move the record back: a resent report from an earlier week settles that report, not
    // the weeks counted since.
    let latest = read_counted(dir).filter(|counted| *counted > counted_in);
    if latest.is_none() && write_week(&dir.join(COUNTED_FILE), &counted_in).is_err() {
        return Outcome::Failed;
    }
    let _ = fs::remove_file(dir.join(PENDING_FILE));
    outcome
}

/// The counter's week of the last settled report.
fn read_counted(dir: &Path) -> Option<String> {
    read_week(&dir.join(COUNTED_FILE))
}

fn read_week(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let week = text.trim();
    is_iso_week(week).then(|| week.to_string())
}

fn write_week(path: &Path, week: &str) -> std::io::Result<()> {
    crate::platform::persist::replace_file(path, week)
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
        .map_or(true, |age| age < CLAIM_TTL)
        && claimed
            .duration_since(now)
            .map_or(true, |ahead| ahead < CLAIM_TTL)
}

fn record_notice_shown(dir: &Path, today: NaiveDate) {
    if crate::platform::fs_security::ensure_private_dir(dir).is_err() {
        return;
    }
    if write_date(&dir.join(NOTICE_FILE), today).is_ok() {
        let _ = fs::remove_file(dir.join(NOTICE_CLAIM_FILE));
    }
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

#[derive(Deserialize)]
struct WeekAnswer {
    week: String,
}

impl Transport for HttpsTransport {
    fn current_week(&self) -> Option<String> {
        let mut response = self.agent.get(WEEK_ENDPOINT).call().ok()?;
        if !response.status().is_success() {
            return None;
        }
        let bytes = response
            .body_mut()
            .with_config()
            .limit(MAX_ANSWER_BYTES)
            .read_to_vec()
            .ok()?;
        serde_json::from_slice::<WeekAnswer>(&bytes)
            .ok()
            .map(|answer| answer.week)
    }

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
    /// first arrived. `week` is its clock; `arrives_in` makes the next reports land in a later
    /// week than the one it just named, as when the week turns over mid-request. `lose_answers`
    /// drops the answer after the counter took the report, which is the case a retry must not
    /// double.
    struct FakeCounter {
        week: RefCell<String>,
        arrives_in: RefCell<Option<String>>,
        offline: RefCell<bool>,
        answer: RefCell<Option<Delivery>>,
        lose_answers: RefCell<bool>,
        tokens: RefCell<HashMap<String, String>>,
        sent: RefCell<Vec<serde_json::Value>>,
    }

    impl FakeCounter {
        fn new() -> Self {
            Self {
                week: RefCell::new("2026-W44".to_string()),
                arrives_in: RefCell::new(None),
                offline: RefCell::new(false),
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
            *self.arrives_in.borrow_mut() = None;
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
        fn current_week(&self) -> Option<String> {
            (!*self.offline.borrow()).then(|| self.week.borrow().clone())
        }

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
            let arrival = self
                .arrives_in
                .borrow()
                .clone()
                .unwrap_or_else(|| self.week.borrow().clone());
            let week = tokens.entry(token).or_insert(arrival).clone();
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
        age_file(&dir.join(NOTICE_CLAIM_FILE), start);

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
        assert!(!dir.join(PENDING_FILE).exists());
    }

    /// The very first report is counted but its answer is lost. The resend learns it was counted
    /// before, which still means this install has been counted, so later weeks age as they should
    /// rather than reporting a new install forever.
    #[test]
    fn a_lost_answer_to_the_first_report_still_starts_the_installs_age() {
        let (_root, dir) = told_on("2026-09-30");
        let counter = FakeCounter::new();
        counter.at("2026-W40");
        *counter.lose_answers.borrow_mut() = true;
        assert_eq!(
            run(&dir, at("2026-10-01T09:00:00Z"), &template(), &counter),
            Outcome::Failed
        );
        *counter.lose_answers.borrow_mut() = false;
        assert_eq!(
            run(&dir, at("2026-10-01T15:00:00Z"), &template(), &counter),
            Outcome::AlreadyCounted
        );

        counter.at("2026-W44");
        assert_eq!(
            run(&dir, at("2026-10-29T09:00:00Z"), &template(), &counter),
            Outcome::Counted
        );
        let sent = counter.sent();
        assert_eq!(sent.last().unwrap()["age"], 3, "{sent:?}");
    }

    /// A client clock two hours fast crosses Sunday midnight while the counter is still on
    /// Sunday. Its check six hours later must not make a second report the counter would count
    /// in the same week; only the counter's week turning over allows one.
    #[test]
    fn a_slightly_fast_client_clock_counts_once_across_sunday_midnight() {
        let (_root, dir) = told_on("2026-10-20");
        let counter = FakeCounter::new();
        counter.at("2026-W44");
        // Counter: Sunday 17:30 and 23:30. Client: 19:30, then Monday 01:30.
        assert_eq!(
            run(&dir, at("2026-11-01T19:30:00Z"), &template(), &counter),
            Outcome::Counted
        );
        assert_eq!(
            run(&dir, at("2026-11-02T01:30:00Z"), &template(), &counter),
            Outcome::AlreadyCounted
        );
        assert_eq!(counter.counted_in("2026-W44"), 1);
        assert_eq!(counter.sent().len(), 1);

        counter.at("2026-W45");
        assert_eq!(
            run(&dir, at("2026-11-02T07:30:00Z"), &template(), &counter),
            Outcome::Counted
        );
        assert_eq!(counter.counted_in("2026-W45"), 1);
    }

    /// A client clock a year ahead, later corrected, changes neither how often the install is
    /// counted nor its age: both follow the counter's weeks.
    #[test]
    fn a_wrong_client_clock_changes_neither_the_count_nor_the_age() {
        let (_root, dir) = told_on("2026-10-20");
        let counter = FakeCounter::new();
        counter.at("2026-W44");
        for time in ["2027-10-28T09:00:00Z", "2027-11-05T09:00:00Z"] {
            run(&dir, at(time), &template(), &counter);
        }
        assert_eq!(counter.counted_in("2026-W44"), 1);

        // Corrected, a week later by the counter's clock, then five weeks on.
        counter.at("2026-W45");
        assert_eq!(
            run(&dir, at("2026-11-03T09:00:00Z"), &template(), &counter),
            Outcome::Counted
        );
        counter.at("2026-W49");
        run(&dir, at("2026-12-01T09:00:00Z"), &template(), &counter);
        let ages: Vec<u64> = counter
            .sent()
            .iter()
            .map(|report| report["age"].as_u64().unwrap())
            .collect();
        assert_eq!(ages, vec![1, 2, 3]);
    }

    /// Without the counter's week there is no telling whether a new week began, so nothing is
    /// sent.
    #[test]
    fn nothing_is_sent_while_the_counters_week_is_unknown() {
        let (_root, dir) = told_on("2026-10-20");
        let counter = FakeCounter::new();
        *counter.offline.borrow_mut() = true;
        assert_eq!(
            run(&dir, at("2026-10-26T09:00:00Z"), &template(), &counter),
            Outcome::Failed
        );
        assert!(counter.sent().is_empty());
        assert!(!dir.join(PENDING_FILE).exists());
    }

    fn age_file(path: &Path, time: DateTime<Utc>) {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::from(time))
            .unwrap();
    }

    /// A client that dies between claiming a report and writing its token leaves an empty claim.
    /// While fresh, it is another client at work; once stale, the next check reports anyway.
    #[test]
    fn an_empty_report_claim_goes_stale_instead_of_blocking_counting() {
        let (_root, dir) = told_on("2026-10-20");
        let claim = dir.join(PENDING_FILE);
        crate::platform::persist::create_private_file(&claim, false).unwrap();
        let counter = FakeCounter::new();
        counter.at("2026-W44");

        let claimed = at("2026-10-29T09:00:00Z");
        age_file(&claim, claimed);
        assert_eq!(
            run(&dir, at("2026-10-29T09:01:00Z"), &template(), &counter),
            Outcome::AlreadyCounted
        );
        assert!(counter.sent().is_empty());

        assert_eq!(
            run(&dir, at("2026-10-29T15:00:00Z"), &template(), &counter),
            Outcome::Counted
        );
        assert_eq!(counter.counted_in("2026-W44"), 1);
    }

    /// The counter took the report but the client could not record it. The token stays, the next
    /// check resends it, and the install is still counted once.
    #[test]
    fn a_failed_local_write_keeps_the_token_for_a_resend() {
        let (_root, dir) = told_on("2026-10-20");
        let counter = FakeCounter::new();
        counter.at("2026-W44");
        // A directory where the record goes makes writing it fail.
        fs::create_dir(dir.join(COUNTED_FILE)).unwrap();
        assert_eq!(
            run(&dir, at("2026-10-26T09:00:00Z"), &template(), &counter),
            Outcome::Failed
        );
        assert!(dir.join(PENDING_FILE).exists());

        fs::remove_dir(dir.join(COUNTED_FILE)).unwrap();
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

    /// Asked just before midnight on Sunday, the counter names its old week, but the report lands
    /// in Monday's. The client records the week that counted it, so Monday does not count again.
    #[test]
    fn the_week_the_counter_names_is_the_week_marked() {
        let (_root, dir) = told_on("2026-10-20");
        let counter = FakeCounter::new();
        counter.at("2026-W44");
        *counter.arrives_in.borrow_mut() = Some("2026-W45".to_string());
        assert_eq!(
            run(&dir, at("2026-11-01T23:59:59Z"), &template(), &counter),
            Outcome::Counted
        );
        counter.at("2026-W45");
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

    /// rozi's real HTTPS client against the live counter: it asks the week, and posts a report
    /// that is invalid on purpose, so the counter refuses it and stores nothing. Run by hand: `cargo test --lib live_counter -- --ignored`.
    #[test]
    #[ignore = "reaches the live counter"]
    fn live_counter_refuses_an_invalid_report_over_https() {
        let body = serde_json::to_vec(&Report {
            token: "not-a-token".to_string(),
            ..template()
        })
        .unwrap();
        let transport = HttpsTransport::new();
        let week = transport
            .current_week()
            .expect("the counter names its week");
        assert!(is_iso_week(&week), "{week}");
        assert_eq!(transport.post(ENDPOINT, &body), Delivery::Rejected);
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
