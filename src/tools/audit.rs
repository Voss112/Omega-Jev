//! Cloudflare security-audit skill: embedded bundle, materialised into the audited project.
//! Third-party content is copied verbatim and never executed by this binary.

use std::path::Path;

use anyhow::{Context, Result, ensure};

macro_rules! bundle {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_str!(concat!("../../assets/skills/security-audit/", $name)))),*]
    };
}

/// Every vendored file, embedded at build time.
pub const FILES: &[(&str, &str)] = bundle![
    "SKILL.md",
    "RECONNAISSANCE.md",
    "HUNTING.md",
    "ATTACK-CLASSES.md",
    "MEMORY-SAFETY-AND-BINARY.md",
    "AI-AND-LLM.md",
    "WEB-PROTOCOL-AND-AUTH.md",
    "CLIENT-SIDE.md",
    "SUPPLY-CHAIN-AND-RELEASE.md",
    "CLOUD-AND-DEPLOYMENT.md",
    "PROTOCOLS-RPC-AND-MESSAGING.md",
    "RESOURCE-EXHAUSTION-AND-AVAILABILITY.md",
    "DATA-ISOLATION-AND-LIFECYCLE.md",
    "DESKTOP-MOBILE-AND-LOCAL-IPC.md",
    "VALIDATION-AND-REPORTING.md",
    "report-schema.json",
    "validate-findings.cjs",
    "validate-coverage-ledger.cjs",
    "LICENSE",
    "SOURCE.md",
];

/// Writes the bundle under `<root>/.omega/skills/security-audit/`, idempotently.
/// Files the audit produced (`findings.json`, `coverage-ledger.json`, reports) are never touched.
pub fn install(root: &Path) -> Result<String> {
    ensure!(root.is_dir(), "dossier de travail introuvable : {}", root.display());
    let dir = root.join(".omega").join("skills").join("security-audit");
    std::fs::create_dir_all(&dir).with_context(|| format!("création {}", dir.display()))?;
    let mut updated = 0;
    for (name, body) in FILES {
        let path = dir.join(name);
        if std::fs::read(&path).ok().as_deref() == Some(body.as_bytes()) {
            continue;
        }
        std::fs::write(&path, body).with_context(|| format!("écriture de {name}"))?;
        updated += 1;
    }
    let node = crate::infra::process::blocking("node").arg("--version").output().is_ok_and(|o| o.status.success());
    let validators = if node {
        "validateurs: node détecté — lance `node validate-findings.cjs findings.json`"
    } else {
        "validateurs: node absent — valide findings.json en lisant report-schema.json"
    };
    Ok(format!(
        "{} fichiers · {}\nSKILL.md, report-schema.json, validateurs .cjs copiés ({updated} mis à jour)\n{validators}",
        FILES.len(),
        dir.display()
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn bundle_is_installed_inside_the_workspace_and_is_idempotent() {
        let root = std::env::temp_dir().join("omega-audit-kit");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let first = super::install(&root).unwrap();
        assert!(first.contains("SKILL.md") && first.contains("report-schema.json"));
        let dir = root.join(".omega/skills/security-audit");
        let skill = std::fs::read_to_string(dir.join("SKILL.md")).unwrap();
        std::fs::write(dir.join("findings.json"), "[]").unwrap();
        super::install(&root).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("SKILL.md")).unwrap(), skill);
        assert_eq!(std::fs::read_to_string(dir.join("findings.json")).unwrap(), "[]", "les findings ne sont jamais écrasés");
        for (name, body) in super::FILES {
            let on_disk = std::fs::read(dir.join(name)).unwrap();
            assert_eq!(on_disk.as_slice(), body.as_bytes(), "{name}");
        }
    }

    #[test]
    fn a_missing_root_is_an_error_not_a_panic() {
        let missing = std::env::temp_dir().join("omega-audit-kit-absent").join("nope");
        assert!(super::install(&missing).is_err());
    }

    #[test]
    fn the_bundle_carries_the_six_phases_and_the_schema() {
        let skill = super::FILES.iter().find(|(n, _)| *n == "SKILL.md").unwrap().1;
        for needle in ["Reconnaissance", "findings.json", "coverage"] {
            assert!(skill.contains(needle), "{needle}");
        }
        assert!(super::FILES.len() >= 20);
    }
}
