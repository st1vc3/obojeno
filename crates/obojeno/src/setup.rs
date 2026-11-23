//! The built-in targets with what was found on this Mac, for the app's
//! first run and Apps settings, and for `obojeno setup`.

use std::collections::BTreeMap;

use obojeno_core::config::{Config, ConfigChange, TargetChange};
use obojeno_core::ipc::{SetupResult, TargetInfo};
use obojeno_core::targets;

use crate::detect::{self, Found, Places};
use crate::link;
use crate::paths::Paths;

pub fn targets(paths: &Paths) -> anyhow::Result<Vec<TargetInfo>> {
    let config = Config::load(&paths.config_file)?;
    let places = Places::of_this_user(&paths.dirs);
    Ok(targets::BUILTIN_NAMES
        .iter()
        .filter_map(|&name| {
            let target = targets::builtin(name)?;
            let found = detect::find(name, &places);
            let configured = config.targets.get(name);
            let options = options(&found, &config, name);
            let plan = link::plan(name, &places, &options);
            let missing = target
                .options
                .iter()
                .find(|(option, default)| default.is_none() && !options.contains_key(*option))
                .map(|(option, _)| (*option).to_owned());
            Some(TargetInfo {
                name: name.to_owned(),
                title: target.title.to_owned(),
                enabled: configured.is_some_and(|t| t.enabled != Some(false)),
                installed: found.installed,
                set_up: found.set_up,
                setup: target.setup.to_owned(),
                snippet: plan
                    .as_ref()
                    .map(|plan| plan.text.clone())
                    .or_else(|| target.snippet.map(str::to_owned)),
                setup_file: plan.map(|plan| places.dirs.abbreviate(&plan.file)),
                options,
                missing,
            })
        })
        .collect())
}

/// Option values found on this Mac; the ones in the config win.
fn options(found: &Found, config: &Config, name: &str) -> BTreeMap<String, String> {
    let mut options = found.options.clone();
    if let Some(configured) = config.targets.get(name) {
        options.extend(configured.options.clone());
    }
    options
}

/// Adds the setup lines of these targets to their apps' configs, skipping
/// the ones set up already.
pub fn link(paths: &Paths, names: &[String]) -> anyhow::Result<Vec<SetupResult>> {
    let config = Config::load(&paths.config_file)?;
    let places = Places::of_this_user(&paths.dirs);
    let abbreviate = |path: &std::path::Path| places.dirs.abbreviate(path);
    Ok(names
        .iter()
        .map(|name| {
            let found = detect::find(name, &places);
            let mut result = SetupResult {
                name: name.clone(),
                file: None,
                backup: None,
                error: None,
            };
            match link::plan(name, &places, &options(&found, &config, name)) {
                None => result.error = Some(format!("obojeno cannot set up {name} by itself")),
                Some(_) if found.set_up == Some(true) => {}
                Some(plan) => match link::add(&plan) {
                    Ok(link::Added::Already) => {}
                    Ok(link::Added::Now { backup }) => {
                        tracing::info!(
                            "{name}: added its setup to {}{}",
                            plan.file.display(),
                            backup
                                .as_ref()
                                .map(|b| format!(", copy of before in {}", b.display()))
                                .unwrap_or_default()
                        );
                        result.file = Some(abbreviate(&plan.file));
                        result.backup = backup.as_deref().map(abbreviate);
                    }
                    Err(e) => result.error = Some(format!("{e:#}")),
                },
            }
            result
        })
        .collect())
}

/// Turns on every installed target the config does not mention yet, with
/// the options found for it. Targets turned off on purpose stay off.
pub fn enable_found(infos: &[TargetInfo], config: &Config) -> ConfigChange {
    let targets: BTreeMap<String, TargetChange> = infos
        .iter()
        .filter(|t| t.installed && t.missing.is_none() && !config.targets.contains_key(&t.name))
        .map(|t| {
            (
                t.name.clone(),
                TargetChange {
                    enabled: true,
                    options: t.options.clone(),
                },
            )
        })
        .collect();
    ConfigChange {
        targets: (!targets.is_empty()).then_some(targets),
        ..ConfigChange::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, installed: bool, missing: Option<&str>) -> TargetInfo {
        TargetInfo {
            name: name.to_owned(),
            title: name.to_owned(),
            enabled: false,
            installed,
            set_up: None,
            setup: String::new(),
            snippet: None,
            setup_file: None,
            options: BTreeMap::from([("vault".to_owned(), "~/Notes".to_owned())]),
            missing: missing.map(str::to_owned),
        }
    }

    #[test]
    fn enables_installed_targets_the_config_does_not_mention() {
        let config = Config::parse("[targets.zen]\nenabled = false\nprofile = \"/p\"\n").unwrap();
        let infos = [
            info("kitty", true, None),
            info("nvim", false, None),
            info("zen", true, None),
            info("obsidian", true, Some("vault")),
        ];
        let change = enable_found(&infos, &config);
        let names: Vec<&String> = change.targets.as_ref().unwrap().keys().collect();
        assert_eq!(
            names,
            ["kitty"],
            "not missing, not off on purpose, not absent"
        );
        assert_eq!(change.targets.unwrap()["kitty"].options["vault"], "~/Notes");
        assert_eq!(enable_found(&[], &config), ConfigChange::default());
    }
}
