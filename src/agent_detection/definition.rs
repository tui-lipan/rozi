//! The declarative agent-definition format.
//!
//! An agent definition says two things: which foreground process *is* this agent, and what its
//! screen looks like in each state. Both are data. The built-in agents in
//! [`super::catalog::BUILTIN_AGENTS`] are written in exactly this format and parsed through
//! exactly this path, so a user-declared agent is never a second-class citizen of a plugin API -
//! there is only one detector, and everything it knows came from a definition.

use regex_lite::Regex;

use crate::session::protocol::{AgentIdentity, DetectedAgentState};

/// How far above the last written row an agent's status chrome can sit.
///
/// The distinction this constant exists for: an agent's transcript quotes its own footer hints
/// constantly ("press esc to interrupt" written *about* interrupting), and only the live chrome at
/// the bottom of the screen speaks for the run.
pub const FOOTER_ROWS: usize = 8;

/// What one matched state rule concludes.
///
/// Variant order is evaluation precedence, and it is the semantic every built-in already had:
/// a blocked prompt outranks any working evidence beneath it, because an agent drawing a spinner
/// *and* an approval dialog is waiting on you either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AgentStateOutcome {
    Blocked,
    Working,
    Idle,
    /// Recognized the agent, learned nothing about its state.
    ///
    /// Not "idle": the caller holds the pane's previous state across this. It is what a view that
    /// replaces the agent's own status chrome - OpenCode's subagent navigator - has to report,
    /// since nothing on such a screen speaks for the run behind it.
    Unknown,
}

impl AgentStateOutcome {
    /// Precedence order, highest first. Evaluation walks this, not declaration order.
    pub const PRECEDENCE: [Self; 4] = [Self::Blocked, Self::Working, Self::Idle, Self::Unknown];

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "blocked" => Some(Self::Blocked),
            "working" => Some(Self::Working),
            "idle" => Some(Self::Idle),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }

    fn resolve(self) -> Option<DetectedAgentState> {
        match self {
            Self::Blocked => Some(DetectedAgentState::Blocked),
            Self::Working => Some(DetectedAgentState::Working),
            Self::Idle => Some(DetectedAgentState::Idle),
            Self::Unknown => None,
        }
    }
}

/// Which pane observation a rule reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MatchSource {
    #[default]
    Screen,
    Title,
}

impl MatchSource {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "screen" => Some(Self::Screen),
            "title" => Some(Self::Title),
            _ => None,
        }
    }
}

/// How much of the screen a rule reads. Meaningless for [`MatchSource::Title`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MatchScope {
    #[default]
    All,
    /// The last [`FOOTER_ROWS`] non-empty lines, which is where agents draw live status chrome.
    Footer,
}

impl MatchScope {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "all" => Some(Self::All),
            "footer" => Some(Self::Footer),
            _ => None,
        }
    }
}

/// One needle. Matched against text that has already been lowercased, so a literal is lowercased
/// when the definition is built and a regex is documented as seeing lowercase input.
#[derive(Clone, Debug)]
pub enum Pattern {
    Literal(String),
    Regex(Regex),
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Literal(a), Self::Literal(b)) => a == b,
            (Self::Regex(a), Self::Regex(b)) => a.as_str() == b.as_str(),
            _ => false,
        }
    }
}

impl Eq for Pattern {}

// Pattern evaluations performed on this thread, for the candidate-gating test.
//
// Thread-local rather than global so it is not polluted by tests running in parallel. Counts the
// screen/title work only - the part whose cost must not scale with how many agents are declared.
#[cfg(test)]
thread_local! {
    pub(crate) static PATTERN_EVALUATIONS: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

impl Pattern {
    pub fn matches(&self, haystack: &str) -> bool {
        #[cfg(test)]
        PATTERN_EVALUATIONS.with(|count| count.set(count.get() + 1));
        match self {
            Self::Literal(needle) => haystack.contains(needle),
            Self::Regex(regex) => regex.is_match(haystack),
        }
    }

    pub fn source(&self) -> &str {
        match self {
            Self::Literal(needle) => needle,
            Self::Regex(regex) => regex.as_str(),
        }
    }
}

/// One state rule: a set of needles over one observation, and what a match concludes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentStateRule {
    pub state: AgentStateOutcome,
    pub source: MatchSource,
    pub scope: MatchScope,
    /// Every one of these must match.
    pub all_of: Vec<Pattern>,
    /// At least one of these must match. An empty list imposes no requirement.
    pub any_of: Vec<Pattern>,
    /// None of these may match.
    pub none_of: Vec<Pattern>,
}

impl AgentStateRule {
    fn matches(&self, screen: &str, footer: &str, title: &str) -> bool {
        let haystack = match (self.source, self.scope) {
            (MatchSource::Title, _) => title,
            (MatchSource::Screen, MatchScope::All) => screen,
            (MatchSource::Screen, MatchScope::Footer) => footer,
        };
        self.all_of.iter().all(|pattern| pattern.matches(haystack))
            && (self.any_of.is_empty()
                || self.any_of.iter().any(|pattern| pattern.matches(haystack)))
            && !self.none_of.iter().any(|pattern| pattern.matches(haystack))
    }
}

/// One agent: how to recognize its process, and how to read its screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentDefinition {
    /// Public identity, built once here and shared with every pane this definition matches, so a
    /// detection sweep clones a pointer rather than the id and label.
    pub identity: std::sync::Arc<AgentIdentity>,
    /// Normalized executable basenames (see [`normalized_name`]).
    pub names: Vec<String>,
    /// Lowercased `/`-separated substrings of an executable path or argv token, for an agent whose
    /// binary name says nothing - a package path such as `@anthropic-ai/claude-code`.
    pub paths: Vec<String>,
    /// Whether the shared base rules apply when none of this agent's own rules match.
    pub base: bool,
    pub states: Vec<AgentStateRule>,
}

impl AgentDefinition {
    /// Stable id: `[a-z0-9_-]+`, or `<extension>.<id>` for one an extension ships.
    pub fn id(&self) -> &str {
        &self.identity.id
    }

    /// What the sidebar shows. Defaults to the id when a definition omits it.
    pub fn label(&self) -> &str {
        &self.identity.label
    }

    /// Whether `value` names this agent's executable.
    pub fn matches_name(&self, normalized: &str) -> bool {
        self.names.iter().any(|name| name == normalized)
    }

    /// Whether `value` - an already lowercased, `/`-separated path or argv token - contains one of
    /// this agent's package markers.
    pub fn matches_path(&self, normalized: &str) -> bool {
        self.paths.iter().any(|path| normalized.contains(path))
    }
}

/// Reduce an executable name to the form [`AgentDefinition::names`] is declared in: lowercase, no
/// directory, no launcher extension.
pub fn normalized_name(value: &str) -> String {
    let mut value = path_basename(value.trim()).to_ascii_lowercase();
    for suffix in [".exe", ".cmd", ".bat", ".ps1", ".js", ".mjs", ".py"] {
        if value.ends_with(suffix) {
            value.truncate(value.len() - suffix.len());
            break;
        }
    }
    value
}

/// The last component of a path written with either separator.
pub fn path_basename(value: &str) -> &str {
    value
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(value)
}

/// Reduce a path or argv token to the form [`AgentDefinition::paths`] is declared in.
pub fn normalized_path(value: &str) -> String {
    value
        .trim_matches(|ch| matches!(ch, '\'' | '"' | ';' | '&'))
        .replace('\\', "/")
        .to_ascii_lowercase()
}

/// The observations one detection sweep hands the rules, already lowercased.
#[derive(Clone, Copy, Debug)]
pub struct AgentDetectionInput<'a> {
    pub screen: &'a str,
    pub title: &'a str,
}

/// Read one definition's rules against one pane observation.
///
/// `base` is appended to the agent's own rules within each precedence tier rather than after all
/// of them, so an agent that adds a `working` rule does not thereby outrank the shared `blocked`
/// vocabulary. `None` is [`AgentStateOutcome::Unknown`]; a screen that matches nothing is idle,
/// because only a positively observed prompt is idle and a definition whose rules all fail has
/// observed its agent sitting at one.
pub fn evaluate(
    definition: &AgentDefinition,
    base: &[AgentStateRule],
    input: AgentDetectionInput<'_>,
) -> Option<DetectedAgentState> {
    let footer = footer_text(input.screen);
    let base = if definition.base { base } else { &[] };
    for outcome in AgentStateOutcome::PRECEDENCE {
        let matched = definition
            .states
            .iter()
            .chain(base)
            .filter(|rule| rule.state == outcome)
            .any(|rule| rule.matches(input.screen, &footer, input.title));
        if matched {
            return outcome.resolve();
        }
    }
    Some(DetectedAgentState::Idle)
}

/// The last [`FOOTER_ROWS`] non-empty lines, in original order.
fn footer_text(screen: &str) -> String {
    let mut lines: Vec<&str> = screen
        .lines()
        .filter(|line| !line.trim().is_empty())
        .rev()
        .take(FOOTER_ROWS)
        .collect();
    lines.reverse();
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(state: AgentStateOutcome, any_of: &[&str]) -> AgentStateRule {
        AgentStateRule {
            state,
            source: MatchSource::Screen,
            scope: MatchScope::All,
            all_of: Vec::new(),
            any_of: any_of
                .iter()
                .map(|value| Pattern::Literal((*value).into()))
                .collect(),
            none_of: Vec::new(),
        }
    }

    fn definition(states: Vec<AgentStateRule>) -> AgentDefinition {
        AgentDefinition {
            identity: AgentIdentity::new("test", "Test").into(),
            names: vec!["test".into()],
            paths: Vec::new(),
            base: true,
            states,
        }
    }

    fn input<'a>(screen: &'a str, title: &'a str) -> AgentDetectionInput<'a> {
        AgentDetectionInput { screen, title }
    }

    #[test]
    fn blocked_outranks_working_regardless_of_declaration_order() {
        let definition = definition(vec![
            rule(AgentStateOutcome::Working, &["spinner"]),
            rule(AgentStateOutcome::Blocked, &["approve?"]),
        ]);
        assert_eq!(
            evaluate(&definition, &[], input("spinner approve?", "")),
            Some(DetectedAgentState::Blocked)
        );
    }

    #[test]
    fn base_rules_share_the_agents_precedence_tiers() {
        // The agent adds working evidence; the base supplies the blocked vocabulary. Appending the
        // base wholesale after the agent's rules would let the working rule win here.
        let base = vec![rule(AgentStateOutcome::Blocked, &["[y/n]"])];
        let definition = definition(vec![rule(AgentStateOutcome::Working, &["■■■■"])]);
        assert_eq!(
            evaluate(&definition, &base, input("■■■■ continue? [y/n]", "")),
            Some(DetectedAgentState::Blocked)
        );
    }

    #[test]
    fn base_false_drops_the_shared_vocabulary() {
        let base = vec![rule(AgentStateOutcome::Blocked, &["[y/n]"])];
        let mut definition = definition(Vec::new());
        definition.base = false;
        assert_eq!(
            evaluate(&definition, &base, input("continue? [y/n]", "")),
            Some(DetectedAgentState::Idle)
        );
    }

    #[test]
    fn unknown_reports_no_evidence_and_a_bare_screen_is_idle() {
        let definition = definition(vec![rule(AgentStateOutcome::Unknown, &["parent up"])]);
        assert_eq!(evaluate(&definition, &[], input("parent up", "")), None);
        assert_eq!(
            evaluate(&definition, &[], input("nothing here", "")),
            Some(DetectedAgentState::Idle)
        );
    }

    #[test]
    fn idle_evidence_outranks_no_evidence() {
        let definition = definition(vec![
            rule(AgentStateOutcome::Unknown, &["navigator"]),
            rule(AgentStateOutcome::Idle, &["composer"]),
        ]);
        assert_eq!(
            evaluate(&definition, &[], input("navigator composer", "")),
            Some(DetectedAgentState::Idle)
        );
    }

    #[test]
    fn footer_scope_reads_only_the_live_chrome() {
        let mut footer_rule = rule(AgentStateOutcome::Working, &["esc to interrupt"]);
        footer_rule.scope = MatchScope::Footer;
        let definition = definition(vec![footer_rule]);
        let transcript = format!("esc to interrupt{}", "\nfiller".repeat(FOOTER_ROWS));
        assert_eq!(
            evaluate(&definition, &[], input(&transcript, "")),
            Some(DetectedAgentState::Idle),
            "a transcript quoting the hint far above the chrome is not a live run"
        );
        assert_eq!(
            evaluate(
                &definition,
                &[],
                input(&format!("{transcript}\nesc to interrupt"), "")
            ),
            Some(DetectedAgentState::Working)
        );
    }

    #[test]
    fn footer_scope_skips_blank_lines_when_counting_rows() {
        let mut footer_rule = rule(AgentStateOutcome::Working, &["esc to interrupt"]);
        footer_rule.scope = MatchScope::Footer;
        let definition = definition(vec![footer_rule]);
        // A terminal grid pads with blank rows; those must not push the chrome out of the footer.
        let screen = format!("esc to interrupt{}", "\n   ".repeat(40));
        assert_eq!(
            evaluate(&definition, &[], input(&screen, "")),
            Some(DetectedAgentState::Working)
        );
    }

    #[test]
    fn all_of_any_of_and_none_of_compose() {
        let composed = AgentStateRule {
            state: AgentStateOutcome::Blocked,
            source: MatchSource::Screen,
            scope: MatchScope::All,
            all_of: vec![Pattern::Literal("esc dismiss".into())],
            any_of: vec![
                Pattern::Literal("enter submit".into()),
                Pattern::Literal("enter toggle".into()),
            ],
            none_of: vec![Pattern::Literal("quoted".into())],
        };
        let definition = definition(vec![composed]);
        assert_eq!(
            evaluate(&definition, &[], input("esc dismiss enter toggle", "")),
            Some(DetectedAgentState::Blocked)
        );
        assert_eq!(
            evaluate(&definition, &[], input("esc dismiss", "")),
            Some(DetectedAgentState::Idle),
            "all_of alone does not satisfy an any_of requirement"
        );
        assert_eq!(
            evaluate(
                &definition,
                &[],
                input("esc dismiss enter submit quoted", "")
            ),
            Some(DetectedAgentState::Idle),
            "none_of vetoes an otherwise matching rule"
        );
    }

    #[test]
    fn a_title_rule_reads_the_title_and_ignores_scope() {
        let mut title_rule = rule(AgentStateOutcome::Working, &["working"]);
        title_rule.source = MatchSource::Title;
        title_rule.scope = MatchScope::Footer;
        let definition = definition(vec![title_rule]);
        assert_eq!(
            evaluate(&definition, &[], input("", "⠋ working")),
            Some(DetectedAgentState::Working)
        );
        assert_eq!(
            evaluate(&definition, &[], input("working", "")),
            Some(DetectedAgentState::Idle)
        );
    }
}
