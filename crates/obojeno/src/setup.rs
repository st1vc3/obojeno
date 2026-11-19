//! The built-in targets with what was found on this Mac, for the app's
//! first run and Apps settings, and for `obojeno setup`.

use std::collections::BTreeMap;

use obojeno_core::config::{Config, ConfigChange, TargetChange};
use obojeno_core::ipc::TargetInfo;
use obojeno_core::targets;

use crate::detect::{self, Places};
use crate::paths::Paths;

pub fn targets(paths: &Paths) -> anyhow::Result<Vec<TargetInfo>> {
    let config = Config::load(&paths.config_file)?;
    let places = Places::of_this_user(&paths.dirs.home);
    Ok(targets::BUILTIN_NAMES
        .iter()
        .filter_map(|&name| {
            let target = targets::builtin(name)?;
            let found = detect::find(name, &places);
            let configured = config.targets.get(name);
            // Values from the config win over the ones found.
            let mut options = found.options;
            if let Some(configured) = configured {
                options.extend(configured.options.clone());
            }
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
                snippet: target.snippet.map(str::to_owned),
                options,
                missing,
            })
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
