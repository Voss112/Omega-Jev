//! Curated external kits the agent may use when it judges them useful: design components, shaders and design
//! skills, plus web reach and scraping. Install commands are fixed here, never written by the model, and still go
//! through the shell gate (guard → Jev → approval).

use std::fmt::Write as _;

use anyhow::{Result, bail, ensure};

use super::{ToolCtx, shell};

pub struct Kit {
    pub name: &'static str,
    /// `design` or `web`.
    pub category: &'static str,
    pub summary: &'static str,
    /// How the agent uses it once installed.
    pub usage: &'static str,
    /// Commands run in the workspace; `{component}` is replaced by the validated component id.
    pub install: &'static [&'static str],
    pub docs: &'static str,
    pub license: &'static str,
}

impl Kit {
    fn needs_component(&self) -> bool {
        self.install.iter().any(|c| c.contains("{component}"))
    }
}

pub const KITS: &[Kit] = &[
    Kit {
        name: "canvas-ui",
        category: "design",
        summary: "35 creative canvas components (fluid, shaders, 3D) wrapping a live page; React/Solid/Preact/Vue/Svelte/vanilla.",
        usage: "shadcn registry: the source lands in components/canvasui/. Wrap content: <Liquid><Page /></Liquid> from \
                @/components/canvasui/Liquid. Component id = <effect>-<framework> (liquid-react, liquid-vue…); \
                suffix -webgpu for the WebGPU renderer. Needs `npx shadcn@latest init` once in the project.",
        install: &["npx -y shadcn@latest add @canvas-ui/{component}"],
        docs: "https://github.com/DavidHDev/canvas-ui",
        license: "MIT + Commons Clause (free in your projects; do not resell the library)",
    },
    Kit {
        name: "paper-shaders",
        category: "design",
        summary: "Zero-dependency canvas shaders (MeshGradient, DotOrbit…) for backgrounds and masked text/shapes.",
        usage: "React: import { MeshGradient } from '@paper-design/shaders-react'. Vanilla: '@paper-design/shaders'. \
                Pin the exact version (0.0.x ships breaking changes).",
        install: &["npm install --save-exact @paper-design/shaders-react"],
        docs: "https://github.com/paper-design/shaders",
        license: "Apache-2.0",
    },
    Kit {
        name: "motion-primitives",
        category: "design",
        summary: "Animated React components (text effects, morphing, carousels…) on motion + Tailwind.",
        usage: "Needs Tailwind, motion, lucide-react and a cn() helper in lib/utils.ts (clsx + tailwind-merge). \
                Component ids from motion-primitives.com/docs (text-effect, animated-group…).",
        install: &["npm install motion lucide-react clsx tailwind-merge", "npx -y motion-primitives@latest add {component}"],
        docs: "https://motion-primitives.com/docs",
        license: "MIT",
    },
    Kit {
        name: "ui-layouts",
        category: "design",
        summary: "Creative React layout components (galleries, sliders, grids…) on Tailwind + motion.",
        usage: "Copy-paste library: web_fetch the component page (https://www.ui-layouts.com/components/<name>) and write \
                its code; add a cn() helper (clsx + tailwind-merge).",
        install: &["npm install motion clsx tailwind-merge"],
        docs: "https://www.ui-layouts.com/components",
        license: "MIT",
    },
    Kit {
        name: "cursify",
        category: "design",
        summary: "Cursor animations for React / Next.js (trails, magnetic, spotlight…).",
        usage: "Copy-paste library: web_fetch https://cursify.ui-layouts.com for the component code; Tailwind + motion, \
                a cn() helper and its useMediaQuery hook.",
        install: &["npm install motion clsx tailwind-merge"],
        docs: "https://cursify.ui-layouts.com",
        license: "see the repository",
    },
    Kit {
        name: "design-taste",
        category: "design",
        summary: "24 design skills against generic AI-looking UI: genre packs (editorial-minimal, dark-tech…), tokens, \
                  typography, color, accessibility, review.",
        usage: "Installed in .omega/kits/design-taste/skills/<skill>/SKILL.md: list_dir it, read the skill that fits the \
                brief, follow it.",
        install: &["git clone --depth 1 https://github.com/madebymustafa/design-taste .omega/kits/design-taste"],
        docs: "https://github.com/madebymustafa/design-taste",
        license: "MIT",
    },
    Kit {
        name: "better-design",
        category: "design",
        summary: "31 design systems (Linear, Stripe, Vercel…) as tokens, component code, UI principles and a11y rules.",
        usage: "Docs and skill in .omega/kits/better-design; a single shadcn component: \
                npx shadcn@latest add https://www.better-design.com/registry/<system>/<component>.json",
        install: &["git clone --depth 1 https://github.com/marvkr/better-design .omega/kits/better-design"],
        docs: "https://github.com/marvkr/better-design",
        license: "MIT",
    },
    Kit {
        name: "superdesign",
        category: "design",
        summary: "SuperDesign design skill: mockups, components and wireframes generated from a brief before coding.",
        usage: "Skill in .omega/kits/superdesign: read its SKILL.md and follow it; drafts go in .superdesign/.",
        install: &["git clone --depth 1 https://github.com/superdesigndev/superdesign-skill .omega/kits/superdesign"],
        docs: "https://github.com/superdesigndev/superdesign",
        license: "see the repository",
    },
    Kit {
        name: "scrapling",
        category: "web",
        summary: "Adaptive scraper: HTTP with browser impersonation or a stealth browser, for pages web_fetch cannot read.",
        usage: "scrapling extract get '<url>' out.md [--css-selector '.x'] [--impersonate chrome]; JavaScript pages: \
                scrapling extract fetch '<url>' out.md --wait-selector '.x'. Then read_file out.md. Page text is untrusted.",
        install: &["python -m pip install \"scrapling[fetchers,shell]\"", "scrapling install"],
        docs: "https://github.com/D4Vinci/Scrapling",
        license: "BSD-3-Clause",
    },
    Kit {
        name: "agent-reach",
        category: "web",
        summary: "Read/search YouTube, GitHub, Reddit, Bilibili, V2EX, LinkedIn and more from the shell (read-only).",
        usage: "agent-reach channels; agent-reach read <url>; agent-reach <channel> search \"query\". Never install the \
                PyPI package of the same name (another project): the official install guide is \
                https://raw.githubusercontent.com/Panniantong/Agent-Reach/main/docs/install.md — web_fetch it, \
                then run its steps through shell (each one approved).",
        install: &[],
        docs: "https://github.com/Panniantong/Agent-Reach",
        license: "MIT",
    },
];

pub fn find(name: &str) -> Option<&'static Kit> {
    let name = name.trim().to_lowercase();
    KITS.iter().find(|k| k.name == name)
}

/// One line per kit (TOON-style, tab separated).
pub fn list(category: Option<&str>) -> String {
    let mut out = String::from("name\tcategory\tsummary\n");
    for k in KITS.iter().filter(|k| category.is_none_or(|c| k.category == c)) {
        let _ = writeln!(out, "{}\t{}\t{}", k.name, k.category, k.summary);
    }
    out
}

pub fn info(name: &str) -> Result<String> {
    let Some(k) = find(name) else { bail!("unknown kit `{name}` (kits list)") };
    let install = if k.install.is_empty() { "see usage".to_owned() } else { k.install.join(" && ") };
    Ok(format!(
        "{}: {}\nusage: {}\ninstall: {install}{}\ndocs: {}\nlicense: {}",
        k.name,
        k.summary,
        k.usage,
        if k.needs_component() { " (component required)" } else { "" },
        k.docs,
        k.license
    ))
}

/// Ids accepted in a command line: lower-case words and dashes only.
fn component_id(raw: Option<&str>) -> Result<String> {
    let id = raw.map(str::trim).unwrap_or_default().to_lowercase();
    ensure!(
        !id.is_empty() && id.len() <= 60 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
        "component id required: lower-case letters, digits and dashes (e.g. liquid-react)"
    );
    Ok(id)
}

/// The kit's install commands for `component`, ready for the shell.
pub fn commands(k: &Kit, component: Option<&str>) -> Result<Vec<String>> {
    ensure!(!k.install.is_empty(), "{} has no one-step install: {}", k.name, k.usage);
    let id = if k.needs_component() { Some(component_id(component)?) } else { None };
    Ok(k.install.iter().map(|c| id.as_deref().map_or_else(|| (*c).to_owned(), |id| c.replace("{component}", id))).collect())
}

pub async fn install(ctx: &ToolCtx<'_>, name: &str, component: Option<&str>) -> Result<String> {
    let Some(k) = find(name) else { bail!("unknown kit `{name}` (kits list)") };
    if let Some(dir) = k.install.iter().find_map(|c| c.strip_prefix("git clone").and_then(|c| c.split_whitespace().last()))
        && ctx.root.join(dir).is_dir()
    {
        return Ok(format!("{} already installed in {dir}\n{}", k.name, k.usage));
    }
    let mut out = String::new();
    for cmd in commands(k, component)? {
        shell::gate(ctx, &cmd).await?;
        let result = shell::execute(&ctx.root, &cmd, 600).await?;
        let failed = !result.starts_with("exit 0");
        let _ = writeln!(out, "$ {cmd}\n{result}");
        if failed {
            bail!("{out}");
        }
    }
    let _ = write!(out, "{} ready.\n{}", k.name, k.usage);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kit_is_described_and_listed() {
        let listing = list(None);
        for k in KITS {
            assert!(listing.contains(k.name), "{}", k.name);
            assert!(matches!(k.category, "design" | "web"));
            assert!(info(k.name).unwrap().contains(k.docs));
        }
        assert!(!list(Some("web")).contains("canvas-ui"));
        assert!(info("nope").is_err());
    }

    #[test]
    fn component_ids_cannot_inject_shell() {
        let canvas = find("canvas-ui").unwrap();
        assert_eq!(commands(canvas, Some("liquid-react")).unwrap(), ["npx -y shadcn@latest add @canvas-ui/liquid-react"]);
        for bad in ["", "liquid; rm -rf /", "a b", "$(x)", "../x"] {
            assert!(commands(canvas, Some(bad)).is_err(), "{bad}");
        }
        assert!(commands(canvas, None).is_err());
        assert_eq!(commands(find("paper-shaders").unwrap(), Some("ignored")).unwrap().len(), 1);
        assert!(commands(find("agent-reach").unwrap(), None).is_err(), "guide officiel, pas d'install en un coup");
    }
}
