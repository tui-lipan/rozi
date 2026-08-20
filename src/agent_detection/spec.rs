//! The `[[agents]]` file model, and its validation into [`AgentDefinition`]s.
//!
//! One path serves three sources - the built-in catalog, `config.toml`, and an `extension.toml` -
//! so an agent Rozi ships and an agent you declare are validated by the same rules and can express
//! the same things.

use serde::Deserialize;

use super::definition::{
    AgentDefinition, AgentStateOutcome, AgentStateRule, MatchScope, MatchSource, Pattern,
    normalized_name, normalized_path,
};

/// A file that carries nothing but agent definitions - the embedded built-in catalog.
///
/// `base` is deliberately not part of the public `[[agents]]` surface: a base rule applies to
/// every agent that has not opted out, so one bad needle there would misread every pane at once.
/// A definition that wants a shared vocabulary of its own restates it, as `claude` does.
#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub(super) struct AgentsFile {
    pub(super) base: Vec<AgentStateSpec>,
    pub(super) agents: Vec<AgentSpec>,
}

#[derive(Clone, Debug, Deserialize, Default, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct AgentSpec {
    pub id: Option<String>,
    pub label: Option<String>,
    /// Whether the shared base rules apply. Defaults to `true`.
    pub base: Option<bool>,
    pub r#match: AgentMatchSpec,
    pub states: Vec<AgentStateSpec>,
}

#[derive(Clone, Debug, Deserialize, Default, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct AgentMatchSpec {
    pub names: Vec<String>,
    pub paths: Vec<String>,
}

impl AgentMatchSpec {
    fn is_empty(&self) -> bool {
        self.names.is_empty() && self.paths.is_empty()
    }
}

#[derive(Clone, Debug, Deserialize, Default, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct AgentStateSpec {
    pub state: Option<String>,
    pub scope: Option<String>,
    pub screen: Option<PatternGroupSpec>,
    pub title: Option<PatternGroupSpec>,
}

#[derive(Clone, Debug, Deserialize, Default, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct PatternGroupSpec {
    pub all_of: Vec<String>,
    pub any_of: Vec<String>,
    pub none_of: Vec<String>,
    /// Whether the needles in this group are regular expressions rather than literals.
    pub regex: bool,
}

impl PatternGroupSpec {
    fn is_empty(&self) -> bool {
        self.all_of.is_empty() && self.any_of.is_empty()
    }
}

/// Where a batch of specs came from, for both id namespacing and legible warnings.
#[derive(Clone, Copy, Debug)]
pub enum AgentOrigin<'a> {
    Builtin,
    Config,
    /// An extension's manifest. Its agent ids are namespaced `<extension>.<id>`, which is also why
    /// an extension can never displace a built-in agent's identity.
    Extension(&'a str),
}

impl AgentOrigin<'_> {
    fn describe(&self) -> String {
        match self {
            Self::Builtin => "built-in agent".to_string(),
            Self::Config => "agent".to_string(),
            Self::Extension(id) => format!("extension `{id}` agent"),
        }
    }

    fn public_id(&self, id: &str) -> String {
        match self {
            Self::Extension(extension) => format!("{extension}.{id}"),
            _ => id.to_string(),
        }
    }
}

fn valid_local_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
}

/// Validate one batch of specs.
///
/// `inherit` supplies match rules to a definition that declares none, which is what lets a config
/// entry retune a built-in agent's screen patterns without restating how to recognize its process.
/// Every rejection is reported; a definition is dropped whole rather than loaded in a surprising
/// partial form, but one invalid *rule* only costs that rule.
pub fn build_definitions(
    raw: Vec<AgentSpec>,
    origin: AgentOrigin<'_>,
    inherit: &[AgentDefinition],
    warnings: &mut Vec<String>,
) -> Vec<AgentDefinition> {
    let noun = origin.describe();
    let mut built: Vec<AgentDefinition> = Vec::new();
    for spec in raw {
        let local = spec.id.as_deref().unwrap_or_default().trim().to_string();
        if !valid_local_id(&local) {
            warnings.push(format!(
                "Ignored {noun} with invalid id `{local}` (expected lowercase letters, digits, `-`, or `_`)"
            ));
            continue;
        }
        let id = origin.public_id(&local);
        if built.iter().any(|existing| existing.id() == id) {
            warnings.push(format!("Ignored duplicate {noun} `{id}`"));
            continue;
        }

        let (names, paths) = if spec.r#match.is_empty() {
            match inherit.iter().find(|candidate| candidate.id() == id) {
                Some(inherited) => (inherited.names.clone(), inherited.paths.clone()),
                None => {
                    warnings.push(format!(
                        "Ignored {noun} `{id}` with no `match.names` or `match.paths`: nothing could ever match it"
                    ));
                    continue;
                }
            }
        } else {
            (
                spec.r#match
                    .names
                    .iter()
                    .map(|name| normalized_name(name))
                    .filter(|name| !name.is_empty())
                    .collect(),
                spec.r#match
                    .paths
                    .iter()
                    .map(|path| normalized_path(path))
                    .filter(|path| !path.is_empty())
                    .collect(),
            )
        };

        let states = spec
            .states
            .into_iter()
            .filter_map(|state| build_state_rule(state, &noun, &id, warnings))
            .collect();

        let label = spec
            .label
            .map(|label| label.trim().to_string())
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| id.clone());
        built.push(AgentDefinition {
            identity: crate::session::protocol::AgentIdentity::new(id, label).into(),
            names,
            paths,
            base: spec.base.unwrap_or(true),
            states,
        });
    }
    built
}

/// Validate the shared base rules. Same rule grammar, no owning agent.
pub(super) fn build_base_rules(
    raw: Vec<AgentStateSpec>,
    warnings: &mut Vec<String>,
) -> Vec<AgentStateRule> {
    raw.into_iter()
        .filter_map(|spec| build_state_rule(spec, "built-in", "base", warnings))
        .collect()
}

fn build_state_rule(
    spec: AgentStateSpec,
    noun: &str,
    id: &str,
    warnings: &mut Vec<String>,
) -> Option<AgentStateRule> {
    let declared = spec.state.as_deref().unwrap_or_default();
    let Some(state) = AgentStateOutcome::parse(declared) else {
        warnings.push(format!(
            "Ignored {noun} `{id}` rule with unknown state `{declared}` (expected blocked, working, idle, or unknown)"
        ));
        return None;
    };
    let (source, group) = match (spec.screen, spec.title) {
        (Some(group), None) => (MatchSource::Screen, group),
        (None, Some(group)) => (MatchSource::Title, group),
        (None, None) => {
            warnings.push(format!(
                "Ignored {noun} `{id}` `{declared}` rule with neither a `screen` nor a `title` table"
            ));
            return None;
        }
        (Some(_), Some(_)) => {
            warnings.push(format!(
                "Ignored {noun} `{id}` `{declared}` rule: set exactly one of `screen` or `title`"
            ));
            return None;
        }
    };
    if group.is_empty() {
        warnings.push(format!(
            "Ignored {noun} `{id}` `{declared}` rule with no `all_of` or `any_of` needles"
        ));
        return None;
    }
    let scope = match spec
        .scope
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => MatchScope::All,
        Some(value) => match MatchScope::parse(value) {
            Some(scope) => scope,
            None => {
                warnings.push(format!(
                    "Ignored {noun} `{id}` `{declared}` rule scope `{value}` (expected all or footer)"
                ));
                MatchScope::All
            }
        },
    };
    if scope == MatchScope::Footer && source == MatchSource::Title {
        warnings.push(format!(
            "Ignored {noun} `{id}` `{declared}` rule scope (only applies to a `screen` table)"
        ));
    }

    let mut compile = |needles: Vec<String>| -> Option<Vec<Pattern>> {
        needles
            .into_iter()
            .map(|needle| build_pattern(&needle, group.regex, noun, id, declared, warnings))
            .collect()
    };
    Some(AgentStateRule {
        state,
        source,
        scope,
        all_of: compile(group.all_of)?,
        any_of: compile(group.any_of)?,
        none_of: compile(group.none_of)?,
    })
}

/// Compile one needle. `None` invalidates the whole rule: a dropped needle would silently widen
/// an `all_of` or narrow a `none_of` into a rule the author did not write.
fn build_pattern(
    needle: &str,
    regex: bool,
    noun: &str,
    id: &str,
    declared: &str,
    warnings: &mut Vec<String>,
) -> Option<Pattern> {
    if needle.is_empty() {
        warnings.push(format!(
            "Ignored {noun} `{id}` `{declared}` rule with an empty needle"
        ));
        return None;
    }
    if !regex {
        // Rules read text the detector has already lowercased, so an author may write a needle in
        // whatever case the agent draws it in.
        return Some(Pattern::Literal(needle.to_lowercase()));
    }
    match regex_lite::Regex::new(needle) {
        Ok(regex) => Some(Pattern::Regex(regex)),
        Err(err) => {
            warnings.push(format!(
                "Ignored {noun} `{id}` `{declared}` rule with invalid regex `{needle}`: {err}"
            ));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Vec<AgentSpec> {
        toml::from_str::<AgentsFile>(text)
            .expect("agents parse")
            .agents
    }

    fn build(text: &str, origin: AgentOrigin<'_>) -> (Vec<AgentDefinition>, Vec<String>) {
        let mut warnings = Vec::new();
        let built = build_definitions(parse(text), origin, &[], &mut warnings);
        (built, warnings)
    }

    #[test]
    fn a_minimal_definition_needs_only_an_id_and_a_match() {
        let (built, warnings) = build(
            r#"
            [[agents]]
            id = "mycoolagent"
            match = { names = ["MCA.exe"], paths = ["@Acme\\MCA"] }
            "#,
            AgentOrigin::Config,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].label(), "mycoolagent", "label defaults to the id");
        assert_eq!(built[0].names, vec!["mca".to_string()]);
        assert_eq!(built[0].paths, vec!["@acme/mca".to_string()]);
        assert!(built[0].base, "the shared vocabulary applies by default");
        assert!(built[0].states.is_empty());
    }

    #[test]
    fn extension_agents_are_namespaced_and_cannot_displace_a_builtin() {
        let (built, warnings) = build(
            r#"
            [[agents]]
            id = "claude"
            match = { names = ["my-claude"] }
            "#,
            AgentOrigin::Extension("git-tools"),
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(built[0].id(), "git-tools.claude");
    }

    #[test]
    fn an_empty_match_inherits_from_the_definition_it_overrides() {
        let mut warnings = Vec::new();
        let inherit = vec![AgentDefinition {
            identity: crate::session::protocol::AgentIdentity::new("claude", "Claude Code").into(),
            names: vec!["claude".into()],
            paths: vec!["@anthropic-ai/claude-code".into()],
            base: true,
            states: Vec::new(),
        }];
        let built = build_definitions(
            parse(
                r#"
                [[agents]]
                id = "claude"
                label = "Claude"
                [[agents.states]]
                state = "working"
                screen = { any_of = ["thinking"] }
                "#,
            ),
            AgentOrigin::Config,
            &inherit,
            &mut warnings,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(built[0].names, vec!["claude".to_string()]);
        assert_eq!(built[0].label(), "Claude");
        assert_eq!(built[0].states.len(), 1);
    }

    #[test]
    fn an_unmatchable_or_misnamed_definition_is_dropped_with_a_reason() {
        let (built, warnings) = build(
            r#"
            [[agents]]
            id = "no-match-at-all"
            [[agents]]
            id = "Bad Id"
            match = { names = ["x"] }
            [[agents]]
            id = "dup"
            match = { names = ["a"] }
            [[agents]]
            id = "dup"
            match = { names = ["b"] }
            "#,
            AgentOrigin::Config,
        );
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].names, vec!["a".to_string()]);
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("nothing could ever match"))
        );
        assert!(warnings.iter().any(|w| w.contains("invalid id")));
        assert!(warnings.iter().any(|w| w.contains("duplicate")));
    }

    #[test]
    fn an_invalid_rule_costs_that_rule_and_not_the_agent() {
        let (built, warnings) = build(
            r#"
            [[agents]]
            id = "a"
            match = { names = ["a"] }
            [[agents.states]]
            state = "sideways"
            screen = { any_of = ["x"] }
            [[agents.states]]
            state = "working"
            [[agents.states]]
            state = "working"
            screen = { any_of = ["x"] }
            title = { any_of = ["y"] }
            [[agents.states]]
            state = "working"
            screen = { none_of = ["only-a-veto"] }
            [[agents.states]]
            state = "blocked"
            screen = { regex = true, any_of = ["("] }
            [[agents.states]]
            state = "working"
            scope = "footer"
            screen = { any_of = ["esc to interrupt"] }
            "#,
            AgentOrigin::Config,
        );
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].states.len(), 1, "only the last rule survives");
        assert_eq!(built[0].states[0].scope, MatchScope::Footer);
        assert_eq!(warnings.len(), 5, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("unknown state")));
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("neither a `screen` nor a `title`"))
        );
        assert!(warnings.iter().any(|w| w.contains("exactly one of")));
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("no `all_of` or `any_of`"))
        );
        assert!(warnings.iter().any(|w| w.contains("invalid regex")));
    }

    #[test]
    fn literal_needles_are_lowercased_because_rules_read_lowercased_text() {
        let (built, _) = build(
            r#"
            [[agents]]
            id = "a"
            match = { names = ["a"] }
            [[agents.states]]
            state = "working"
            screen = { any_of = ["Esc To Interrupt"] }
            "#,
            AgentOrigin::Config,
        );
        assert_eq!(built[0].states[0].any_of[0].source(), "esc to interrupt");
    }

    #[test]
    fn a_scope_on_a_title_rule_warns_rather_than_silently_reading_the_footer() {
        let (built, warnings) = build(
            r#"
            [[agents]]
            id = "a"
            match = { names = ["a"] }
            [[agents.states]]
            state = "working"
            scope = "footer"
            title = { any_of = ["working"] }
            "#,
            AgentOrigin::Config,
        );
        assert_eq!(built[0].states.len(), 1);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("only applies to a `screen` table"))
        );
    }
}
