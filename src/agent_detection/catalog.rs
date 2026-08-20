//! The resolved set of agent definitions one detection sweep reads.
//!
//! Built-ins are parsed from an embedded file in the public format and then merged with whatever
//! `config.toml` and installed extensions declared. Merge order is what gives an override its
//! meaning: user definitions sit ahead of the built-ins, so declaring `id = "claude"` in your own
//! config replaces Rozi's Claude Code entry outright rather than competing with it.

use super::definition::{AgentDefinition, AgentStateRule, normalized_name, normalized_path};
use super::spec::{AgentOrigin, AgentsFile, build_base_rules, build_definitions};

/// The built-in catalog source, in the same format `[[agents]]` accepts.
const BUILTIN_AGENTS: &str = include_str!("builtin.toml");

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentCatalog {
    definitions: Vec<AgentDefinition>,
    base: Vec<AgentStateRule>,
}

impl AgentCatalog {
    /// The agents Rozi ships, with no user contributions.
    pub fn builtin() -> Self {
        Self::with_definitions(Vec::new())
    }

    /// The built-in catalog, with `user` definitions taking precedence.
    ///
    /// A user definition whose id matches a built-in replaces it; one with a new id is simply
    /// added. Either way it is consulted first, so a definition can claim an executable name a
    /// built-in also lists.
    pub fn with_definitions(user: Vec<AgentDefinition>) -> Self {
        let (builtins, base) = builtin_source();
        let mut definitions = user;
        for builtin in builtins {
            if !definitions
                .iter()
                .any(|existing| existing.id() == builtin.id())
            {
                definitions.push(builtin);
            }
        }
        Self { definitions, base }
    }

    /// The built-in definitions alone, for resolving an override's inherited `match` rules.
    pub fn builtin_definitions() -> Vec<AgentDefinition> {
        builtin_source().0
    }

    /// A shared built-in catalog, parsed once per process.
    ///
    /// The default a server without user definitions runs on, and the fallback wherever no
    /// resolved catalog is available. Sessions with contributions build their own.
    pub fn shared_builtin() -> std::sync::Arc<Self> {
        static BUILTIN: std::sync::OnceLock<std::sync::Arc<AgentCatalog>> =
            std::sync::OnceLock::new();
        BUILTIN
            .get_or_init(|| std::sync::Arc::new(Self::builtin()))
            .clone()
    }

    pub fn base(&self) -> &[AgentStateRule] {
        &self.base
    }

    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    pub fn by_id(&self, id: &str) -> Option<&AgentDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.id() == id)
    }

    /// The agent an executable name or path names, by basename.
    pub fn by_name(&self, value: &str) -> Option<&AgentDefinition> {
        let normalized = normalized_name(value);
        if normalized.is_empty() {
            return None;
        }
        self.definitions
            .iter()
            .find(|definition| definition.matches_name(&normalized))
    }

    /// The agent an argv token names: its basename first, then a package-path marker inside it.
    ///
    /// The second tier is what recognizes an agent invoked through a runtime, where the executable
    /// is `node` and the only evidence is the script path it was handed.
    pub fn by_path(&self, value: &str) -> Option<&AgentDefinition> {
        let token = value.trim_matches(|ch| matches!(ch, '\'' | '"' | ';' | '&'));
        if let Some(definition) = self.by_name(token) {
            return Some(definition);
        }
        let normalized = normalized_path(token);
        self.definitions
            .iter()
            .find(|definition| definition.matches_path(&normalized))
    }
}

/// Parse the embedded catalog.
///
/// Panics only on a malformed embedded file, which is a build defect rather than a runtime
/// condition - [`builtin_catalog_is_valid`] fails first.
fn builtin_source() -> (Vec<AgentDefinition>, Vec<AgentStateRule>) {
    let file: AgentsFile =
        toml::from_str(BUILTIN_AGENTS).expect("embedded built-in agent catalog parses");
    let mut warnings = Vec::new();
    let base = build_base_rules(file.base, &mut warnings);
    let definitions = build_definitions(file.agents, AgentOrigin::Builtin, &[], &mut warnings);
    debug_assert!(
        warnings.is_empty(),
        "embedded built-in agent catalog is invalid: {warnings:?}"
    );
    (definitions, base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_detection::definition::AgentStateOutcome;

    #[test]
    fn builtin_catalog_is_valid() {
        let file: AgentsFile =
            toml::from_str(BUILTIN_AGENTS).expect("embedded built-in agent catalog parses");
        let mut warnings = Vec::new();
        let base = build_base_rules(file.base, &mut warnings);
        let definitions = build_definitions(file.agents, AgentOrigin::Builtin, &[], &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(!base.is_empty());
        assert!(definitions.len() > 20, "{} agents", definitions.len());
        assert!(
            definitions
                .iter()
                .all(|definition| !definition.names.is_empty() || !definition.paths.is_empty())
        );
    }

    #[test]
    fn names_paths_and_wrappers_resolve_to_the_right_agent() {
        let catalog = AgentCatalog::builtin();
        for (value, id) in [
            ("claude", "claude"),
            ("claude-code", "claude"),
            ("open-code", "opencode"),
            ("opencode-tui", "opencode"),
            ("ghcs", "github-copilot"),
            ("antigravity-cli", "antigravity"),
            ("qoderclicn", "qoder-cli"),
            ("aider-chat", "aider"),
        ] {
            assert_eq!(
                catalog.by_name(value).map(|agent| agent.id()),
                Some(id),
                "{value}"
            );
        }
        assert!(catalog.by_name("bash").is_none());
        for (value, id) in [
            (
                "/opt/node_modules/@anthropic-ai/claude-code/cli.js",
                "claude",
            ),
            ("@openai/codex", "codex"),
            ("/usr/lib/opencode-ai/index.js", "opencode"),
            ("/work/target/release/opencode-tui", "opencode"),
        ] {
            assert_eq!(
                catalog.by_path(value).map(|agent| agent.id()),
                Some(id),
                "{value}"
            );
        }
    }

    #[test]
    fn a_user_definition_replaces_the_builtin_sharing_its_id() {
        let mut warnings = Vec::new();
        let user = build_definitions(
            toml::from_str::<AgentsFile>(
                r#"
                [[agents]]
                id = "claude"
                label = "Claude (mine)"
                [[agents.states]]
                state = "working"
                screen = { any_of = ["cogitating"] }
                "#,
            )
            .expect("parses")
            .agents,
            AgentOrigin::Config,
            &AgentCatalog::builtin_definitions(),
            &mut warnings,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        let catalog = AgentCatalog::with_definitions(user);
        let claude = catalog.by_name("claude").expect("claude resolves");
        assert_eq!(claude.label(), "Claude (mine)");
        assert_eq!(claude.states.len(), 1, "the built-in rules are replaced");
        assert_eq!(claude.states[0].state, AgentStateOutcome::Working);
        assert_eq!(
            catalog.by_id("claude").map(|agent| agent.label()),
            Some("Claude (mine)"),
            "only one claude survives the merge"
        );
    }

    #[test]
    fn a_user_definition_can_claim_a_name_a_builtin_also_lists() {
        let mut warnings = Vec::new();
        let user = build_definitions(
            toml::from_str::<AgentsFile>(
                r#"
                [[agents]]
                id = "my-wrapper"
                label = "Wrapper"
                match = { names = ["claude"] }
                "#,
            )
            .expect("parses")
            .agents,
            AgentOrigin::Config,
            &[],
            &mut warnings,
        );
        let catalog = AgentCatalog::with_definitions(user);
        assert_eq!(
            catalog.by_name("claude").map(|agent| agent.id()),
            Some("my-wrapper")
        );
        assert!(
            catalog.by_id("claude").is_some(),
            "the built-in is still present, just outranked"
        );
    }
}
