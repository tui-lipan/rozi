//! The anonymous weekly usage count.
//!
//! Once per ISO week, one client on each machine posts a small report to the usage counter: the
//! release, channel, OS, architecture, install channel, and a coarse install-age bucket. Nothing
//! in the report identifies the machine or the user. There is no install ID, and the counter
//! stores only per-week aggregate counters, never the request or its address. Which week it is
//! comes from the server's clock, not the report.
//!
//! The rules that make the number trustworthy, and the user's consent meaningful, all live here:
//!
//! - A machine is counted at most once a week. The week's marker file is claimed atomically, so of
//!   several clients open at once exactly one sends. A send that fails for a transient reason
//!   releases the marker so a later check retries; a report the server rejects is not retried.
//! - Nothing is sent before the user has been told. The first time this runs on a machine it only
//!   records the date and asks the client to show the notice; the first report goes out on a later
//!   day, which leaves time to opt out.
//! - `[updates] count_usage = false`, `[updates] check = false`, `DO_NOT_TRACK`, a CI environment,
//!   and a debug build each keep the machine out of the count.
//!
//! The wire format is a public contract with the counter in `tools/usage-counter/`. Add fields
//! rather than change them, and bump [`SCHEMA_VERSION`] only for a change the counter must
//! recognise.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::time::Duration;

use chrono::{Datelike, NaiveDate, Utc};
use serde::Serialize;
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
const NOTICE_FILE: &str = "notice";
const FIRST_COUNTED_FILE: &str = "first-counted";
const WEEK_PREFIX: &str = "week-";
const DATE_FORMAT: &str = "%Y-%m-%d";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const NIGHTLY_COMMIT: Option<&str> = option_env!("ROZI_NIGHTLY_COMMIT");

/// One week's report. Every value comes from a short fixed vocabulary, so the report cannot carry
/// anything about the machine beyond what the field names say.
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
}

/// What a check did, for the caller and for tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// First run on this machine: nothing was sent, and the client should show the notice.
    NoticeDue,
    /// The notice went up today; the first report waits for a later day.
    WaitingAfterNotice,
    /// This machine was already counted this week, possibly by another client.
    AlreadyCounted,
    /// The report was accepted.
    Counted,
    /// The counter refused the report. Retrying this week would only repeat the refusal.
    Rejected,
    /// The report did not get through; a later check this week tries again.
    Failed,
}

/// How a post ended, as far as retrying is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delivery {
    Accepted,
    /// A 4xx answer: the counter understood the request and will not take it.
    Rejected,
    /// No answer, or a server error.
    Failed,
}

/// The network edge, replaced in tests.
pub(crate) trait Transport {
    fn post(&self, url: &str, body: &[u8]) -> Delivery;
}

/// Why this machine might be excluded regardless of configuration.
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

/// Run one check from a client's update loop. `enabled` is the client's own gate: release checks
/// allowed, `[updates] check` and `[updates] count_usage` all on.
///
/// Blocks on the network, so the caller is a worker thread.
pub(crate) fn report_if_due(link: &CommandLink<Msg>, enabled: bool) {
    if !enabled || Exclusions::from_process().any() {
        return;
    }
    let env = crate::platform::paths::PlatformEnv::from_process();
    let Some(state_dir) = crate::platform::paths::state_dir_if_available(&env) else {
        return;
    };
    let report = Report::current(crate::platform::install_source::detect_current());
    let outcome = run(
        &state_dir.join(DIR_NAME),
        Utc::now().date_naive(),
        report,
        &HttpsTransport::new(),
    );
    if outcome == Outcome::NoticeDue {
        link.send(Msg::UsageCountNotice);
    }
}

impl Report {
    /// This build's report; [`run`] fills in the age.
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

/// Install-age bucket from the date this machine was first counted. Coarse on purpose: it tells new
/// installs from returning ones without dating any of them.
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

fn week_marker(today: NaiveDate) -> String {
    let week = today.iso_week();
    format!("{WEEK_PREFIX}{}-W{:02}", week.year(), week.week())
}

/// One check against the state in `dir`, as of `today`. `report.age` is replaced with the bucket
/// this machine is in.
pub(crate) fn run(
    dir: &Path,
    today: NaiveDate,
    mut report: Report,
    transport: &impl Transport,
) -> Outcome {
    if crate::platform::fs_security::ensure_private_dir(dir).is_err() {
        return Outcome::Failed;
    }
    match notice_date(dir, today) {
        NoticeState::JustShown => return Outcome::NoticeDue,
        NoticeState::Taken => return Outcome::WaitingAfterNotice,
        NoticeState::Shown(shown) if shown >= today => return Outcome::WaitingAfterNotice,
        NoticeState::Shown(_) => {}
        NoticeState::Unavailable => return Outcome::Failed,
    }

    let marker = dir.join(week_marker(today));
    match crate::platform::persist::create_private_file(&marker, false) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => return Outcome::AlreadyCounted,
        Err(_) => return Outcome::Failed,
    }

    let first_counted = read_date(&dir.join(FIRST_COUNTED_FILE));
    report.age = age_bucket(first_counted.unwrap_or(today), today);
    let Ok(body) = serde_json::to_vec(&report) else {
        let _ = fs::remove_file(&marker);
        return Outcome::Failed;
    };
    match transport.post(ENDPOINT, &body) {
        Delivery::Accepted => {
            if first_counted.is_none() {
                let _ = write_date(&dir.join(FIRST_COUNTED_FILE), today);
            }
            remove_stale_markers(dir, &marker);
            Outcome::Counted
        }
        Delivery::Rejected => {
            remove_stale_markers(dir, &marker);
            Outcome::Rejected
        }
        Delivery::Failed => {
            let _ = fs::remove_file(&marker);
            Outcome::Failed
        }
    }
}

enum NoticeState {
    /// This call created the notice record, so this client shows the notice.
    JustShown,
    /// Another client created it a moment ago and is showing the notice.
    Taken,
    Shown(NaiveDate),
    Unavailable,
}

/// When the notice was shown, claiming the right to show it if nobody has.
fn notice_date(dir: &Path, today: NaiveDate) -> NoticeState {
    let path = dir.join(NOTICE_FILE);
    match crate::platform::persist::create_private_file(&path, false) {
        Ok(mut file) => {
            let _ = file.write_all(today.format(DATE_FORMAT).to_string().as_bytes());
            NoticeState::JustShown
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => match read_date(&path) {
            Some(shown) => NoticeState::Shown(shown),
            // Either the creator has not written the date yet, or the file is damaged. Repair it
            // as shown today: the notice was shown at some point, and the first report still
            // waits a day.
            None => {
                let _ = crate::platform::persist::replace_file(
                    &path,
                    today.format(DATE_FORMAT).to_string(),
                );
                NoticeState::Taken
            }
        },
        Err(_) => NoticeState::Unavailable,
    }
}

fn read_date(path: &Path) -> Option<NaiveDate> {
    let text = fs::read_to_string(path).ok()?;
    NaiveDate::parse_from_str(text.trim(), DATE_FORMAT).ok()
}

fn write_date(path: &Path, date: NaiveDate) -> std::io::Result<()> {
    crate::platform::persist::replace_file(path, date.format(DATE_FORMAT).to_string())
}

/// Drop markers from earlier weeks so the directory holds one at most.
fn remove_stale_markers(dir: &Path, current: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        let is_marker = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(WEEK_PREFIX));
        if is_marker && path != current {
            let _ = fs::remove_file(path);
        }
    }
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

impl Transport for HttpsTransport {
    fn post(&self, url: &str, body: &[u8]) -> Delivery {
        let response = self
            .agent
            .post(url)
            .header("content-type", "application/json")
            .send(body);
        match response.map(|response| response.status().as_u16()) {
            Ok(200..=299) => Delivery::Accepted,
            // A timeout or a rate limit says nothing about the report itself.
            Ok(408 | 429) => Delivery::Failed,
            Ok(400..=499) => Delivery::Rejected,
            _ => Delivery::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeCounter {
        answer: Delivery,
        sent: RefCell<Vec<serde_json::Value>>,
    }

    impl FakeCounter {
        fn answering(answer: Delivery) -> Self {
            Self {
                answer,
                sent: RefCell::new(Vec::new()),
            }
        }

        fn sent(&self) -> Vec<serde_json::Value> {
            self.sent.borrow().clone()
        }
    }

    impl Transport for FakeCounter {
        fn post(&self, url: &str, body: &[u8]) -> Delivery {
            assert_eq!(url, ENDPOINT);
            self.sent
                .borrow_mut()
                .push(serde_json::from_slice(body).expect("the report is JSON"));
            self.answer
        }
    }

    fn day(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, DATE_FORMAT).unwrap()
    }

    fn report() -> Report {
        Report::current(InstallSource::Managed)
    }

    fn scratch() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    /// The notice goes up first, and nothing is sent until a later day.
    #[test]
    fn nothing_is_sent_before_a_day_has_passed_since_the_notice() {
        let root = scratch();
        let dir = root.path().join(DIR_NAME);
        let counter = FakeCounter::answering(Delivery::Accepted);

        assert_eq!(
            run(&dir, day("2026-10-29"), report(), &counter),
            Outcome::NoticeDue
        );
        assert_eq!(
            run(&dir, day("2026-10-29"), report(), &counter),
            Outcome::WaitingAfterNotice
        );
        assert!(counter.sent().is_empty());

        assert_eq!(
            run(&dir, day("2026-10-30"), report(), &counter),
            Outcome::Counted
        );
        assert_eq!(counter.sent().len(), 1);
    }

    #[test]
    fn a_machine_is_counted_once_per_iso_week() {
        let root = scratch();
        let dir = root.path().join(DIR_NAME);
        let counter = FakeCounter::answering(Delivery::Accepted);
        run(&dir, day("2026-10-20"), report(), &counter);

        // 2026-10-26 is a Monday: Monday to Sunday is one week, the next Monday a new one.
        for date in ["2026-10-26", "2026-10-28", "2026-11-01"] {
            run(&dir, day(date), report(), &counter);
        }
        assert_eq!(counter.sent().len(), 1);
        assert_eq!(
            run(&dir, day("2026-11-02"), report(), &counter),
            Outcome::Counted
        );
        assert_eq!(counter.sent().len(), 2);

        let markers: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with(WEEK_PREFIX))
            .collect();
        assert_eq!(markers, vec!["week-2026-W45".to_string()]);
    }

    #[test]
    fn a_failed_send_is_retried_and_a_rejected_one_is_not() {
        let root = scratch();
        let dir = root.path().join(DIR_NAME);
        run(
            &dir,
            day("2026-10-20"),
            report(),
            &FakeCounter::answering(Delivery::Accepted),
        );

        let offline = FakeCounter::answering(Delivery::Failed);
        assert_eq!(
            run(&dir, day("2026-10-26"), report(), &offline),
            Outcome::Failed
        );
        let online = FakeCounter::answering(Delivery::Accepted);
        assert_eq!(
            run(&dir, day("2026-10-26"), report(), &online),
            Outcome::Counted
        );

        let refusing = FakeCounter::answering(Delivery::Rejected);
        assert_eq!(
            run(&dir, day("2026-11-02"), report(), &refusing),
            Outcome::Rejected
        );
        assert_eq!(
            run(&dir, day("2026-11-03"), report(), &refusing),
            Outcome::AlreadyCounted
        );
        assert_eq!(refusing.sent().len(), 1);
    }

    /// The age bucket counts from the first accepted report, not from the notice or a failed send.
    #[test]
    fn age_moves_through_its_buckets_from_the_first_accepted_report() {
        let root = scratch();
        let dir = root.path().join(DIR_NAME);
        run(
            &dir,
            day("2026-01-01"),
            report(),
            &FakeCounter::answering(Delivery::Accepted),
        );
        run(
            &dir,
            day("2026-01-05"),
            report(),
            &FakeCounter::answering(Delivery::Failed),
        );

        let counter = FakeCounter::answering(Delivery::Accepted);
        for date in ["2026-01-06", "2026-01-13", "2026-02-10", "2026-06-23"] {
            run(&dir, day(date), report(), &counter);
        }
        let ages: Vec<u64> = counter
            .sent()
            .iter()
            .map(|report| report["age"].as_u64().unwrap())
            .collect();
        assert_eq!(ages, vec![1, 2, 3, 4]);
    }

    /// The report is the whole contract with the counter: these fields, and nothing else.
    #[test]
    fn the_report_carries_only_its_fixed_fields() {
        let root = scratch();
        let dir = root.path().join(DIR_NAME);
        let counter = FakeCounter::answering(Delivery::Accepted);
        run(&dir, day("2026-10-01"), report(), &counter);
        run(&dir, day("2026-10-02"), report(), &counter);

        let sent = counter.sent();
        let object = sent[0].as_object().unwrap();
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "age", "app", "arch", "channel", "install", "os", "schema", "version"
            ]
        );
        assert_eq!(object["schema"], 1);
        assert_eq!(object["app"], "rozi");
        assert_eq!(object["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(object["install"], "managed");
        assert_eq!(object["os"], std::env::consts::OS);

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
        let root = scratch();
        let dir = root.path().join(DIR_NAME);
        crate::platform::fs_security::ensure_private_dir(&dir).unwrap();
        fs::write(dir.join(NOTICE_FILE), "").unwrap();
        let counter = FakeCounter::answering(Delivery::Accepted);

        assert_eq!(
            run(&dir, day("2026-10-29"), report(), &counter),
            Outcome::WaitingAfterNotice
        );
        assert_eq!(
            run(&dir, day("2026-10-30"), report(), &counter),
            Outcome::Counted
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
