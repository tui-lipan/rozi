//! Screen evidence: what each supported agent actually draws, and the state Rozi reads from it.
//!
//! A detection rule is a claim about somebody else's user interface. Nothing in this repository can
//! verify that claim - only a capture of the real program can - so the corpus under
//! `tests/fixtures/agents/` holds those captures and this module asserts the definitions still
//! read them correctly. That is the whole split: the fixtures are evidence, the rules are the
//! theory, and a change to either has to keep answering for the other.
//!
//! Adding an agent therefore means adding a screen, not just a table. See the corpus README for
//! how to capture one.

use super::{AgentCatalog, AgentDetectionInput, definition::evaluate};
use crate::session::protocol::DetectedAgentState;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Agents whose real screens predate this corpus and are asserted inline in `mod.rs` instead.
///
/// Those screens were written from live panes and carry cross-agent assertions a per-agent fixture
/// cannot express - that Claude's dialog shape must not classify an OpenCode screen, say - so they
/// stay where they are and this names them. An agent listed here may still gain a fixture for
/// screens the inline tests do not cover; Claude did, and left this list when it did.
const EVIDENCE_IN_UNIT_TESTS: &[&str] = &["opencode"];

/// Agents that ship without a screen fixture, and why.
///
/// Every entry is a detection Rozi offers on the strength of the shared vocabulary alone, with
/// nobody having watched the tool to confirm it. Removing a name from this list - by capturing the
/// tool in each state it can be in - is the work; adding one is admitting a gap, not resolving it.
/// The list is asserted to be exactly the set of agents with no fixture, so it cannot quietly rot
/// into a list of agents that were covered years ago.
const AWAITING_EVIDENCE: &[&str] = &[
    "gemini",
    "devin",
    "omp",
    "mastracode",
    "kimi",
    "kiro",
    "droid",
    "amp",
    "hermes",
    "kilo",
    "qoder-cli",
    "aider",
];

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agents")
}

/// Every fixture file in the corpus, as `(agent id, parsed fixture)`.
fn fixtures() -> Vec<(String, Fixture)> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(corpus()).expect("the fixture corpus exists") {
        let path = entry.expect("readable corpus entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("toml") {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("a fixture file is named for its agent")
            .to_string();
        let text = std::fs::read_to_string(&path).expect("readable fixture");
        let fixture: Fixture =
            toml::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        found.push((id, fixture));
    }
    found.sort_by(|(left, _), (right, _)| left.cmp(right));
    found
}

/// One agent's captured screens. Mirrors `tests/fixtures/agents/README.md`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    /// Whether these screens were seen in a real pane or reasoned out from somewhere else. A
    /// `derived` fixture proves the rules are self-consistent and nothing more, so it is a
    /// placeholder for a capture rather than a substitute for one.
    source: Source,
    #[allow(dead_code, reason = "provenance is for the reader, not the assertions")]
    captured_at: String,
    #[serde(default)]
    #[allow(dead_code, reason = "provenance is for the reader, not the assertions")]
    notes: Option<String>,
    #[serde(default, rename = "case")]
    cases: Vec<Case>,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Source {
    Capture,
    Derived,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    state: String,
    #[serde(default)]
    title: String,
    screen: String,
}

impl Case {
    /// The state this case says the screen means. `unknown` is the outcome that declines to speak,
    /// which the server resolves against whatever it was holding.
    fn expected(&self) -> Option<DetectedAgentState> {
        match self.state.as_str() {
            "working" => Some(DetectedAgentState::Working),
            "blocked" => Some(DetectedAgentState::Blocked),
            "idle" => Some(DetectedAgentState::Idle),
            "unknown" => None,
            other => panic!("unknown expected state `{other}` in case `{}`", self.name),
        }
    }
}

/// The corpus is the regression suite for every rule in `builtin.toml`: a rule that stops reading
/// the screen it was written for, or starts reading one it was written to ignore, fails here.
#[test]
fn screens_read_the_states_their_fixtures_claim() {
    let catalog = AgentCatalog::builtin();
    let mut checked = 0;
    for (id, fixture) in fixtures() {
        let definition = catalog
            .by_id(&id)
            .unwrap_or_else(|| panic!("{id}.toml names no agent in builtin.toml"));
        assert!(
            !fixture.cases.is_empty(),
            "{id}: a fixture with no cases is evidence of nothing"
        );
        let mut names = std::collections::HashSet::new();
        for case in &fixture.cases {
            assert!(
                names.insert(case.name.as_str()),
                "{id}: duplicate case name `{}`",
                case.name
            );
            // Detection lowercases before matching, so needles stay lowercase; a fixture is raw
            // screen text and has to go through the same door.
            let read = evaluate(
                definition,
                catalog.base(),
                AgentDetectionInput {
                    screen: &case.screen.to_lowercase(),
                    title: &case.title.to_lowercase(),
                },
            );
            assert_eq!(
                read,
                case.expected(),
                "{id}/{}: this screen reads as {read:?}",
                case.name
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "the corpus must not be empty");
}

/// An agent Rozi claims to support with nobody having ever watched it run is a guess on the
/// sidebar. This does not force the guess to be right - it forces it to be admitted.
#[test]
fn every_shipped_agent_has_screen_evidence() {
    let covered = fixtures()
        .into_iter()
        .map(|(id, _)| id)
        .collect::<std::collections::HashSet<_>>();
    let shipped = AgentCatalog::builtin_definitions()
        .iter()
        .map(|definition| definition.id().to_string())
        .collect::<std::collections::HashSet<_>>();

    let accounted = |id: &str| {
        covered.contains(id)
            || AWAITING_EVIDENCE.contains(&id)
            || EVIDENCE_IN_UNIT_TESTS.contains(&id)
    };
    let missing = shipped
        .iter()
        .filter(|id| !accounted(id))
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "no screen fixture for {missing:?} - capture one, or add the id to AWAITING_EVIDENCE"
    );

    for id in AWAITING_EVIDENCE.iter().chain(EVIDENCE_IN_UNIT_TESTS) {
        assert!(
            shipped.contains(*id),
            "`{id}` is excused from the corpus but is not an agent Rozi ships"
        );
        assert!(
            !covered.contains(*id),
            "`{id}` has a fixture now - drop it from the list excusing it"
        );
    }
}

/// A fixture nobody captured is a placeholder, and placeholders are supposed to leave.
///
/// Deliberately not a failure: a derived screen still pins the rule it was written beside, which
/// beats leaving the rule untested. It is listed so the gap stays countable.
#[test]
fn derived_fixtures_are_named_as_the_placeholders_they_are() {
    let derived = fixtures()
        .into_iter()
        .filter(|(_, fixture)| fixture.source == Source::Derived)
        .map(|(id, _)| id)
        .collect::<Vec<_>>();
    assert_eq!(
        derived,
        Vec::<String>::new(),
        "update this list when a derived fixture is added or replaced by a capture"
    );
}
