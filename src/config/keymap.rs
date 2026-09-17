//! What a `[keys]` binding *means*, separate from the chords it currently resolves to.
//!
//! A binding is written against the input scheme or as a physical chord:
//!
//! | Source          | Meaning                                     |
//! | --------------- | ------------------------------------------- |
//! | `enter`         | [`BindingExpr::Scheme`]: prefix and Mod forms |
//! | `scheme:ctrl-t` | [`BindingExpr::Scheme`] for a modified key  |
//! | `prefix:enter`  | [`BindingExpr::Prefix`]: prefix form only   |
//! | `mod:enter`     | [`BindingExpr::Mod`]: Mod form only         |
//! | `ctrl-b enter`  | [`BindingExpr::Literal`]: exactly that       |
//!
//! [`Config::key_sources`] keeps these expressions; `Config::key_overrides` is derived from them for
//! the current [`InputConfig`]. Anything that edits bindings works on the expressions, so changing
//! the prefix or Mod moves every scheme-relative binding and leaves literal ones where they are.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use tui_lipan::prelude::KeyBinding;

use super::input::is_bare_key_step;
use super::schema::{Config, InputConfig, WmModifier};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindingExpr {
    /// One key step through both halves of the scheme.
    Scheme(KeyBinding),
    /// One key step after the prefix.
    Prefix(KeyBinding),
    /// One key step held with Mod. Dormant while `modifier_shortcuts` is off.
    Mod(KeyBinding),
    /// A physical binding that never follows the scheme.
    Literal(KeyBinding),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindingExprError {
    /// `scheme:`, `prefix:`, or `mod:` was given something other than one key step.
    NotOneStep,
    Invalid,
}

/// Builds an expression from its marker's one key step.
type StepExpr = fn(KeyBinding) -> BindingExpr;

const MARKERS: [(&str, StepExpr); 3] = [
    ("scheme:", BindingExpr::Scheme),
    ("prefix:", BindingExpr::Prefix),
    ("mod:", BindingExpr::Mod),
];

impl BindingExpr {
    pub fn parse(source: &str) -> Result<Self, BindingExprError> {
        let source = source.trim();
        for (marker, expr) in MARKERS {
            if let Some(step) = source.strip_prefix(marker) {
                return match KeyBinding::from_str(step) {
                    Ok(step) if step.step_count() == 1 => Ok(expr(step)),
                    _ => Err(BindingExprError::NotOneStep),
                };
            }
        }
        let binding = KeyBinding::from_str(source).map_err(|_| BindingExprError::Invalid)?;
        Ok(Self::from_binding(binding))
    }

    /// The expression a plainly written or recorded binding stands for: a bare command key follows
    /// the scheme, anything carrying Ctrl, Alt, or Super is literal. This never infers `prefix:` or
    /// `mod:` from a chord that merely happens to contain the current prefix or Mod.
    pub fn from_binding(binding: KeyBinding) -> Self {
        if is_bare_key_step(&binding.canonical_lowercase()) {
            Self::Scheme(binding)
        } else {
            Self::Literal(binding)
        }
    }

    /// The `[keys]` spelling, which parses back to `self`. See [`KeyBinding::to_source`].
    pub fn source(&self) -> String {
        match self {
            Self::Scheme(step) => {
                let step = step.to_source();
                if is_bare_key_step(&step) {
                    step
                } else {
                    format!("scheme:{step}")
                }
            }
            Self::Prefix(step) => format!("prefix:{}", step.to_source()),
            Self::Mod(step) => format!("mod:{}", step.to_source()),
            Self::Literal(binding) => binding.to_source(),
        }
    }

    /// Each resolved chord with the half of the scheme it came from: `prefix`, `mod`, or
    /// `literal`.
    pub fn resolve_halves(&self, input: &InputConfig) -> Vec<(KeyBinding, &'static str)> {
        match self {
            Self::Scheme(step) => prefix_chord(input, step)
                .map(|chord| (chord, "prefix"))
                .into_iter()
                .chain(mod_chord(input, step).map(|chord| (chord, "mod")))
                .collect(),
            Self::Prefix(step) => prefix_chord(input, step)
                .map(|chord| (chord, "prefix"))
                .into_iter()
                .collect(),
            Self::Mod(step) => mod_chord(input, step)
                .map(|chord| (chord, "mod"))
                .into_iter()
                .collect(),
            Self::Literal(binding) => vec![(binding.clone(), "literal")],
        }
    }

    pub fn resolve(&self, input: &InputConfig) -> Vec<KeyBinding> {
        match self {
            Self::Scheme(step) => prefix_chord(input, step)
                .into_iter()
                .chain(mod_chord(input, step))
                .collect(),
            Self::Prefix(step) => prefix_chord(input, step).into_iter().collect(),
            Self::Mod(step) => mod_chord(input, step).into_iter().collect(),
            Self::Literal(binding) => vec![binding.clone()],
        }
    }

    /// Display alternatives. A scheme binding reads as its command key, the way built-in defaults
    /// do; everything else reads as the chord it resolves to.
    pub fn display_parts(&self, input: &InputConfig) -> Vec<String> {
        match self {
            Self::Scheme(step) => vec![step.label()],
            _ => self.resolve(input).iter().map(KeyBinding::label).collect(),
        }
    }

    /// What remains once every chord conflicting with `taken` is given away. Splitting a scheme
    /// binding keeps the surviving half scheme-relative instead of freezing it as a physical chord.
    pub fn without(&self, input: &InputConfig, taken: &[KeyBinding]) -> Option<Self> {
        let lost = |binding: Option<KeyBinding>| {
            binding.is_some_and(|binding| taken.iter().any(|taken| binding.conflicts_with(taken)))
        };
        match self {
            Self::Scheme(step) => match (
                lost(prefix_chord(input, step)),
                lost(mod_chord(input, step)),
            ) {
                (false, false) => Some(self.clone()),
                (true, true) => None,
                (true, false) => Some(Self::Mod(step.clone())),
                (false, true) => Some(Self::Prefix(step.clone())),
            },
            _ => {
                let resolved = self.resolve(input);
                (!resolved.into_iter().any(|binding| lost(Some(binding)))).then(|| self.clone())
            }
        }
    }

    /// The `prefix:` expression this literal is structurally equal to under `input`.
    pub fn prefix_equivalent(&self, input: &InputConfig) -> Option<Self> {
        let Self::Literal(binding) = self else {
            return None;
        };
        let canonical = binding.canonical_lowercase();
        let (first, rest) = canonical.split_once(' ')?;
        if first != input.prefix.canonical_lowercase() {
            return None;
        }
        let step = KeyBinding::from_str(rest)
            .ok()
            .filter(|step| step.step_count() == 1)?;
        let equivalent = Self::Prefix(step);
        (equivalent.resolve(input) == [binding.clone()]).then_some(equivalent)
    }

    /// The `mod:` expression this literal is structurally equal to under `input`'s modifier.
    pub fn mod_equivalent(&self, input: &InputConfig) -> Option<Self> {
        let Self::Literal(binding) = self else {
            return None;
        };
        let canonical = binding.canonical_lowercase();
        if canonical.contains(' ') {
            return None;
        }
        let modifier = modifier_canonical_token(input.modifier);
        let tokens = canonical.split('+').collect::<Vec<_>>();
        let (key, modifiers) = tokens.split_last()?;
        if !modifiers.contains(&modifier) {
            return None;
        }
        let rest = modifiers
            .iter()
            .filter(|token| **token != modifier)
            .copied()
            .chain([*key])
            .collect::<Vec<_>>()
            .join("+");
        let step = KeyBinding::from_str(&rest).ok()?;
        let equivalent = Self::Mod(step);
        let active = InputConfig {
            modifier_shortcuts: true,
            ..input.clone()
        };
        (equivalent.resolve(&active) == [binding.clone()]).then_some(equivalent)
    }
}

/// How the modifier is spelled inside `canonical_lowercase` output.
fn modifier_canonical_token(modifier: WmModifier) -> &'static str {
    match modifier {
        WmModifier::Alt => "alt",
        WmModifier::Super => "super",
    }
}

pub fn prefix_chord(input: &InputConfig, step: &KeyBinding) -> Option<KeyBinding> {
    KeyBinding::from_str(&format!(
        "{} {}",
        input.prefix.canonical_lowercase(),
        step.canonical_lowercase()
    ))
    .ok()
}

pub fn mod_chord(input: &InputConfig, step: &KeyBinding) -> Option<KeyBinding> {
    if !input.modifier_shortcuts {
        return None;
    }
    KeyBinding::from_str(&format!(
        "{}-{}",
        input.modifier.token(),
        step.canonical_lowercase()
    ))
    .ok()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverrideMode {
    /// The entry replaces the action's defaults.
    Replace,
    /// `{ add = … }`: the entry extends the action's defaults.
    Add,
}

/// One `[keys]` action entry as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyOverrideSpec {
    pub mode: OverrideMode,
    pub bindings: Vec<BindingExpr>,
}

impl KeyOverrideSpec {
    pub fn replace(bindings: Vec<BindingExpr>) -> Self {
        Self {
            mode: OverrideMode::Replace,
            bindings,
        }
    }

    /// Every expression the action answers to, defaults included for an additive entry.
    pub fn expressions(&self, defaults: &[BindingExpr]) -> Vec<BindingExpr> {
        match self.mode {
            OverrideMode::Replace => self.bindings.clone(),
            OverrideMode::Add => defaults.iter().chain(&self.bindings).cloned().collect(),
        }
    }

    pub fn resolve(&self, input: &InputConfig, defaults: &[BindingExpr]) -> Vec<KeyBinding> {
        resolve_all(&self.expressions(defaults), input)
    }

    /// The value written under `[keys]`.
    pub fn toml_value(&self) -> String {
        let sources = self
            .bindings
            .iter()
            .map(|expr| toml::Value::String(expr.source()).to_string())
            .collect::<Vec<_>>();
        let list = match sources.as_slice() {
            [] => "[]".to_string(),
            [one] => one.clone(),
            many => format!("[{}]", many.join(", ")),
        };
        match self.mode {
            OverrideMode::Replace => list,
            OverrideMode::Add => format!("{{ add = {list} }}"),
        }
    }
}

/// Resolve expressions in order, dropping duplicate chords.
pub fn resolve_all(exprs: &[BindingExpr], input: &InputConfig) -> Vec<KeyBinding> {
    let mut bindings = Vec::new();
    for binding in exprs.iter().flat_map(|expr| expr.resolve(input)) {
        if !bindings.contains(&binding) {
            bindings.push(binding);
        }
    }
    bindings
}

/// Derive the effective override chords from the source entries.
pub fn resolve_key_overrides(
    sources: &HashMap<String, KeyOverrideSpec>,
    input: &InputConfig,
) -> HashMap<String, Vec<KeyBinding>> {
    sources
        .iter()
        .map(|(id, spec)| {
            let defaults = crate::commands::default_binding_exprs(id).unwrap_or_default();
            (id.clone(), spec.resolve(input, &defaults))
        })
        .collect()
}

/// The expressions an action currently answers to, as the editor must see them: its `[keys]`
/// entry, or its defaults when it has none.
pub fn effective_expressions(config: &Config, id: &str) -> Vec<BindingExpr> {
    let defaults = crate::commands::default_binding_exprs(id).unwrap_or_default();
    match config.key_sources.get(id) {
        Some(spec) => spec.expressions(&defaults),
        None => {
            let mut exprs = defaults;
            if id == "paste" {
                exprs.push(BindingExpr::Literal(crate::commands::paste_direct_binding()));
            }
            exprs
        }
    }
}

/// Who answers to a claimed chord.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum KeymapOwner {
    /// A core command following its defaults: a built-in action, a generated workspace command,
    /// or prefix forwarding.
    Core(String),
    /// An action or named command bound in `[keys]`.
    Override(String),
    /// An inline `[keys]` command, by its index in [`Config::user_commands`].
    UserCommand(usize),
}

impl KeymapOwner {
    /// The command registry id this owner registers under.
    pub fn registry_id(&self, config: &Config) -> String {
        match self {
            Self::Core(id) => id.clone(),
            Self::Override(id) if config.commands.iter().any(|command| &command.id == id) => {
                format!("command.{id}")
            }
            Self::Override(id) => id.clone(),
            Self::UserCommand(index) => format!("user.{index}"),
        }
    }

    /// The `[keys]` id the editor can rewrite, if any.
    pub fn config_id(&self, config: &Config) -> Option<String> {
        match self {
            Self::Core(id) => crate::commands::default_binding_exprs(id).map(|_| id.clone()),
            Self::Override(id) => Some(id.clone()),
            Self::UserCommand(_) => None,
        }
        .filter(|id| {
            crate::commands::default_binding_exprs(id).is_some()
                || config.commands.iter().any(|command| &command.id == id)
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeymapClaim {
    pub binding: KeyBinding,
    pub owner: KeymapOwner,
    /// The expression and scheme half that produced the chord, such as `enter@prefix`. Unlike the
    /// chord, this stays the same when Prefix or Mod changes.
    pub source: String,
}

impl KeymapClaim {
    fn from_expr(owner: &KeymapOwner, expr: &BindingExpr, input: &InputConfig) -> Vec<Self> {
        let source = expr.source();
        expr.resolve_halves(input)
            .into_iter()
            .map(|(binding, half)| Self {
                binding,
                owner: owner.clone(),
                source: format!("{source}@{half}"),
            })
            .collect()
    }

    fn identity(&self) -> (String, String) {
        (format!("{:?}", self.owner), self.source.clone())
    }
}

/// Every chord in the global command domain that is not negotiable: core defaults, `[keys]`
/// entries, and inline commands. Extension defaults and suggestions are deliberately absent; they
/// resolve against these claims and lose.
pub fn hard_claims(config: &Config) -> Vec<KeymapClaim> {
    let input = &config.input;
    let mut claims = Vec::new();
    for (id, expr) in crate::commands::core_default_exprs() {
        if !config.key_overrides.contains_key(&id) {
            claims.extend(KeymapClaim::from_expr(&KeymapOwner::Core(id), &expr, input));
        }
    }
    if let Some(binding) = crate::commands::prefix_forward_chord(input) {
        claims.push(KeymapClaim {
            binding,
            owner: KeymapOwner::Core(crate::commands::FORWARD_PREFIX_COMMAND_ID.to_string()),
            source: "prefix-forward".to_string(),
        });
    }
    let mut overridden = config.key_overrides.iter().collect::<Vec<_>>();
    overridden.sort_by(|left, right| left.0.cmp(right.0));
    for (id, bindings) in overridden {
        let owner = KeymapOwner::Override(id.clone());
        let exprs = match config.key_sources.get(id) {
            Some(spec) => {
                spec.expressions(&crate::commands::default_binding_exprs(id).unwrap_or_default())
            }
            // An override set without a source (tests building `Config` by hand) is literal.
            None => bindings.iter().cloned().map(BindingExpr::Literal).collect(),
        };
        for expr in exprs {
            claims.extend(KeymapClaim::from_expr(&owner, &expr, input));
        }
    }
    for (index, command) in config.user_commands.iter().enumerate() {
        claims.extend(KeymapClaim::from_expr(
            &KeymapOwner::UserCommand(index),
            &command.trigger,
            input,
        ));
    }
    claims
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeymapCollision {
    pub first: KeymapClaim,
    pub second: KeymapClaim,
}

impl KeymapCollision {
    /// Which two sources collide, independent of claim order and of the chords they currently
    /// resolve to.
    fn identity(&self) -> ((String, String), (String, String)) {
        let (first, second) = (self.first.identity(), self.second.identity());
        if first <= second {
            (first, second)
        } else {
            (second, first)
        }
    }
}

/// Pairs of claims from different owners where one chord would fire before the other can: equal
/// sequences, or one a prefix of the other. Aliases of one owner never collide.
pub fn collisions(claims: &[KeymapClaim]) -> Vec<KeymapCollision> {
    let mut found = Vec::new();
    for (index, claim) in claims.iter().enumerate() {
        for other in claims.iter().skip(index + 1) {
            if claim.owner != other.owner && claim.binding.conflicts_with(&other.binding) {
                found.push(KeymapCollision {
                    first: claim.clone(),
                    second: other.clone(),
                });
            }
        }
    }
    found
}

/// Collisions in `candidate` that `baseline` did not already have.
///
/// A collision is identified by the two owners *and* the source expressions on each side. That
/// tolerates a pre-existing hand-written conflict, even once a Prefix change moves both of its
/// chords, while still catching a new collision between the same two commands on another binding.
pub fn new_collisions(baseline: &Config, candidate: &Config) -> Vec<KeymapCollision> {
    let existing = collisions(&hard_claims(baseline))
        .iter()
        .map(KeymapCollision::identity)
        .collect::<HashSet<_>>();
    collisions(&hard_claims(candidate))
        .into_iter()
        .filter(|collision| !existing.contains(&collision.identity()))
        .collect()
}

/// A config file change the keybinding editor makes in one validated write.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeymapEdit {
    /// `Some` writes the entry; `None` removes it so the action follows its defaults.
    pub overrides: Vec<(String, Option<KeyOverrideSpec>)>,
    pub prefix: Option<KeyBinding>,
    pub modifier: Option<WmModifier>,
    pub modifier_shortcuts: Option<bool>,
}

/// Rewrite every literal that `equivalent` can express scheme-relatively, keeping each entry's mode.
pub fn converted_literals(
    config: &Config,
    equivalent: impl Fn(&BindingExpr) -> Option<BindingExpr>,
) -> Vec<(String, KeyOverrideSpec)> {
    let mut converted = config
        .key_sources
        .iter()
        .filter(|(_, spec)| spec.bindings.iter().any(|expr| equivalent(expr).is_some()))
        .map(|(id, spec)| {
            let bindings = spec
                .bindings
                .iter()
                .map(|expr| equivalent(expr).unwrap_or_else(|| expr.clone()))
                .collect();
            (
                id.clone(),
                KeyOverrideSpec {
                    mode: spec.mode,
                    bindings,
                },
            )
        })
        .collect::<Vec<_>>();
    converted.sort_by(|left, right| left.0.cmp(&right.0));
    converted
}

/// How many literal bindings `equivalent` could convert.
pub fn convertible_literal_count(
    config: &Config,
    equivalent: impl Fn(&BindingExpr) -> Option<BindingExpr>,
) -> usize {
    config
        .key_sources
        .values()
        .flat_map(|spec| &spec.bindings)
        .filter(|expr| equivalent(expr).is_some())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(text: &str) -> KeyBinding {
        KeyBinding::from_str(text).unwrap()
    }

    fn expr(text: &str) -> BindingExpr {
        BindingExpr::parse(text).unwrap()
    }

    fn bindings(texts: &[&str]) -> Vec<KeyBinding> {
        texts.iter().map(|text| binding(text)).collect()
    }

    fn input(prefix: &str, modifier: WmModifier, modifier_shortcuts: bool) -> InputConfig {
        InputConfig {
            prefix: binding(prefix),
            modifier,
            modifier_shortcuts,
            ..InputConfig::default()
        }
    }

    #[test]
    fn source_forms_parse_to_their_meaning() {
        assert_eq!(expr("enter"), BindingExpr::Scheme(binding("enter")));
        assert_eq!(
            expr("scheme:ctrl-t"),
            BindingExpr::Scheme(binding("ctrl-t"))
        );
        assert_eq!(expr("prefix:enter"), BindingExpr::Prefix(binding("enter")));
        assert_eq!(expr("mod:enter"), BindingExpr::Mod(binding("enter")));
        assert_eq!(
            expr("ctrl-b enter"),
            BindingExpr::Literal(binding("ctrl-b enter"))
        );
        assert_eq!(expr("alt-w"), BindingExpr::Literal(binding("alt-w")));
        for marker in ["scheme:", "prefix:", "mod:"] {
            assert_eq!(
                BindingExpr::parse(&format!("{marker}ctrl-a x")),
                Err(BindingExprError::NotOneStep)
            );
        }
        assert_eq!(
            BindingExpr::parse("not-a-real-key"),
            Err(BindingExprError::Invalid)
        );
    }

    #[test]
    fn every_form_round_trips_through_its_source() {
        for source in [
            "enter",
            "shift-w",
            "scheme:ctrl-t",
            "prefix:enter",
            "mod:shift-w",
            "ctrl-b enter",
            "alt-w",
        ] {
            let parsed = expr(source);
            assert_eq!(expr(&parsed.source()), parsed, "{source}");
        }
    }

    #[test]
    fn each_form_resolves_against_the_scheme() {
        let default = InputConfig::default();
        assert_eq!(
            expr("enter").resolve(&default),
            bindings(&["ctrl-a enter", "alt-enter"])
        );
        assert_eq!(
            expr("scheme:ctrl-t").resolve(&default),
            bindings(&["ctrl-a ctrl-t", "alt-ctrl-t"])
        );
        assert_eq!(expr("prefix:x").resolve(&default), bindings(&["ctrl-a x"]));
        assert_eq!(expr("mod:x").resolve(&default), bindings(&["alt-x"]));
        assert_eq!(expr("ctrl-b x").resolve(&default), bindings(&["ctrl-b x"]));

        let mod_off = input("ctrl-a", WmModifier::Alt, false);
        assert_eq!(expr("enter").resolve(&mod_off), bindings(&["ctrl-a enter"]));
        assert!(expr("mod:x").resolve(&mod_off).is_empty());
        assert_eq!(expr("prefix:x").resolve(&mod_off), bindings(&["ctrl-a x"]));
    }

    #[test]
    fn a_prefix_change_moves_scheme_and_prefix_forms_only() {
        let moved = input("ctrl-b", WmModifier::Alt, true);
        assert_eq!(expr("x").resolve(&moved), bindings(&["ctrl-b x", "alt-x"]));
        assert_eq!(expr("prefix:x").resolve(&moved), bindings(&["ctrl-b x"]));
        assert_eq!(expr("mod:x").resolve(&moved), bindings(&["alt-x"]));
        assert_eq!(expr("ctrl-a x").resolve(&moved), bindings(&["ctrl-a x"]));
    }

    #[test]
    fn a_mod_change_moves_scheme_and_mod_forms_only() {
        let moved = input("ctrl-a", WmModifier::Super, true);
        assert_eq!(
            expr("x").resolve(&moved),
            bindings(&["ctrl-a x", "super-x"])
        );
        assert_eq!(expr("prefix:x").resolve(&moved), bindings(&["ctrl-a x"]));
        assert_eq!(expr("mod:x").resolve(&moved), bindings(&["super-x"]));
        assert_eq!(expr("alt-x").resolve(&moved), bindings(&["alt-x"]));
    }

    #[test]
    fn taking_half_of_a_scheme_binding_keeps_the_other_half_scheme_relative() {
        let default = InputConfig::default();
        let scheme = expr("enter");
        assert_eq!(
            scheme.without(&default, &bindings(&["alt-enter"])),
            Some(expr("prefix:enter"))
        );
        assert_eq!(
            scheme.without(&default, &bindings(&["ctrl-a enter"])),
            Some(expr("mod:enter"))
        );
        assert_eq!(
            scheme.without(&default, &bindings(&["ctrl-a enter", "alt-enter"])),
            None
        );
        assert_eq!(
            scheme.without(&default, &bindings(&["alt-x"])),
            Some(scheme.clone())
        );
        // A longer sequence starting with the chord takes it too.
        assert_eq!(
            scheme.without(&default, &bindings(&["alt-enter x"])),
            Some(expr("prefix:enter"))
        );
        assert_eq!(
            expr("prefix:enter").without(&default, &bindings(&["ctrl-a enter"])),
            None
        );
        assert_eq!(
            expr("mod:enter").without(&default, &bindings(&["alt-enter"])),
            None
        );
        assert_eq!(expr("alt-w").without(&default, &bindings(&["alt-w"])), None);
    }

    #[test]
    fn literals_convert_only_when_structurally_equivalent() {
        let default = InputConfig::default();
        assert_eq!(
            expr("ctrl-a x").prefix_equivalent(&default),
            Some(expr("prefix:x"))
        );
        assert_eq!(
            expr("ctrl-a shift-x").prefix_equivalent(&default),
            Some(expr("prefix:shift-x"))
        );
        assert_eq!(expr("ctrl-b x").prefix_equivalent(&default), None);
        assert_eq!(expr("ctrl-a x y").prefix_equivalent(&default), None);
        assert_eq!(expr("prefix:x").prefix_equivalent(&default), None);

        assert_eq!(expr("alt-w").mod_equivalent(&default), Some(expr("mod:w")));
        assert_eq!(
            expr("ctrl-alt-w").mod_equivalent(&default),
            Some(expr("mod:ctrl-w"))
        );
        assert_eq!(expr("super-w").mod_equivalent(&default), None);
        assert_eq!(expr("ctrl-w").mod_equivalent(&default), None);
        assert_eq!(expr("alt-w x").mod_equivalent(&default), None);

        let super_mod = input("ctrl-a", WmModifier::Super, true);
        assert_eq!(
            expr("super-w").mod_equivalent(&super_mod),
            Some(expr("mod:w"))
        );
    }

    #[test]
    fn written_bindings_use_tui_lipan_source_spelling() {
        for (source, written) in [
            ("super-w", "super-w"),
            ("cmd+w", "super-w"),
            ("meta-shift-p", "super-shift-p"),
            ("shift-ctrl-a", "ctrl-shift-a"),
            ("alt-shift-/", "alt-shift-/"),
            ("ctrl-a ctrl-minus", "ctrl-a ctrl-minus"),
            ("ctrl--", "ctrl-minus"),
            ("ctrl-+", "ctrl-plus"),
            ("-", "minus"),
            ("+", "plus"),
            (",", "comma"),
            ("ctrl-,", "ctrl-comma"),
            ("ctrl-space", "ctrl-space"),
            ("f5", "f5"),
            ("?", "?"),
        ] {
            assert_eq!(binding(source).to_source(), written, "{source}");
            assert_eq!(binding(written), binding(source), "{written} parses back");
        }
        assert_eq!(expr("scheme:super-t").source(), "scheme:super-t");
        assert_eq!(expr("mod:shift-w").source(), "mod:shift-w");
        assert_eq!(expr("prefix:ctrl-t").source(), "prefix:ctrl-t");
    }

    #[test]
    fn specs_serialize_to_parseable_toml() {
        let replace = KeyOverrideSpec::replace(vec![expr("enter"), expr("mod:x")]);
        assert_eq!(replace.toml_value(), r#"["enter", "mod:x"]"#);
        assert_eq!(KeyOverrideSpec::replace(vec![]).toml_value(), "[]");
        assert_eq!(
            KeyOverrideSpec::replace(vec![expr("prefix:x")]).toml_value(),
            r#""prefix:x""#
        );
        let add = KeyOverrideSpec {
            mode: OverrideMode::Add,
            bindings: vec![expr("super-enter")],
        };
        assert_eq!(add.toml_value(), r#"{ add = "super-enter" }"#);
    }

    #[test]
    fn collisions_cover_equal_and_prefix_sequences_between_owners() {
        let claim = |text: &str, owner: KeymapOwner| KeymapClaim {
            binding: binding(text),
            owner,
            source: format!("{text}@literal"),
        };
        let a = KeymapOwner::Override("spawn".into());
        let b = KeymapOwner::Override("close".into());
        let found = collisions(&[
            claim("ctrl-a x", a.clone()),
            claim("ctrl-a x", b.clone()),
            claim("ctrl-a y", a.clone()),
            claim("ctrl-a y z", b.clone()),
            claim("ctrl-a q", a.clone()),
            claim("ctrl-a q", a.clone()),
        ]);
        assert_eq!(found.len(), 2, "{found:?}");
    }
}
