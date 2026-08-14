use crate::config::file::ServiceFileConfig;
use crate::config::schema::{ServiceConfig, ServiceRestart};
use std::collections::HashSet;

pub(crate) fn build_services(
    raw: Vec<ServiceFileConfig>,
    warnings: &mut Vec<String>,
) -> Vec<ServiceConfig> {
    let mut services = Vec::new();
    let mut seen_names = HashSet::new();

    for service in raw {
        let Some(name) = service
            .name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
        else {
            warnings.push("service missing required field `name`; skipped".to_string());
            continue;
        };

        if !seen_names.insert(name.clone()) {
            warnings.push(format!("duplicate service name `{name}`; skipped"));
            continue;
        }

        let Some(run) = service
            .run
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty())
        else {
            warnings.push(format!(
                "service `{name}` missing required field `run`; skipped"
            ));
            continue;
        };

        let restart = match service.restart.as_deref().map(str::trim) {
            None | Some("") => ServiceRestart::OnFailure,
            Some("on-failure") => ServiceRestart::OnFailure,
            Some("always") => ServiceRestart::Always,
            Some("never") => ServiceRestart::Never,
            Some(other) => {
                warnings.push(format!(
                    "service `{name}` has unknown restart policy `{other}`; skipped"
                ));
                continue;
            }
        };

        let cwd = service
            .cwd
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty());

        services.push(ServiceConfig {
            name,
            run,
            cwd,
            restart,
            env: service.env,
        });
    }

    services
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_services_build_cleanly() {
        let mut warnings = Vec::new();
        let raw = vec![ServiceFileConfig {
            name: Some("test-svc".to_string()),
            run: Some("echo 1".to_string()),
            cwd: Some("/tmp".to_string()),
            restart: Some("always".to_string()),
            env: [("K".to_string(), "V".to_string())].into_iter().collect(),
        }];
        let services = build_services(raw, &mut warnings);
        assert!(warnings.is_empty());
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].name, "test-svc");
        assert_eq!(services[0].restart, ServiceRestart::Always);
        assert_eq!(services[0].cwd.as_deref(), Some("/tmp"));
    }

    #[test]
    fn missing_name_warns_and_drops() {
        let mut warnings = Vec::new();
        let raw = vec![ServiceFileConfig {
            name: None,
            run: Some("echo 1".to_string()),
            cwd: None,
            restart: None,
            env: Default::default(),
        }];
        let services = build_services(raw, &mut warnings);
        assert_eq!(services.len(), 0);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("missing required field `name`"));
    }

    #[test]
    fn duplicate_name_warns_and_drops() {
        let mut warnings = Vec::new();
        let raw = vec![
            ServiceFileConfig {
                name: Some("dup".to_string()),
                run: Some("echo 1".to_string()),
                cwd: None,
                restart: None,
                env: Default::default(),
            },
            ServiceFileConfig {
                name: Some("dup".to_string()),
                run: Some("echo 2".to_string()),
                cwd: None,
                restart: None,
                env: Default::default(),
            },
        ];
        let services = build_services(raw, &mut warnings);
        assert_eq!(services.len(), 1);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("duplicate service name `dup`"));
    }

    #[test]
    fn missing_run_warns_and_drops() {
        let mut warnings = Vec::new();
        let raw = vec![ServiceFileConfig {
            name: Some("norun".to_string()),
            run: None,
            cwd: None,
            restart: None,
            env: Default::default(),
        }];
        let services = build_services(raw, &mut warnings);
        assert_eq!(services.len(), 0);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("missing required field `run`"));
    }

    #[test]
    fn unknown_restart_warns_and_drops() {
        let mut warnings = Vec::new();
        let raw = vec![ServiceFileConfig {
            name: Some("badrestart".to_string()),
            run: Some("echo 1".to_string()),
            cwd: None,
            restart: Some("invalid".to_string()),
            env: Default::default(),
        }];
        let services = build_services(raw, &mut warnings);
        assert_eq!(services.len(), 0);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("unknown restart policy `invalid`"));
    }
}
