//! `computer` tool: the agent looks at the screen and drives the mouse and keyboard.

use std::time::Duration;

use anyhow::{Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::Value;

use super::{ToolCtx, ToolOutput};
use crate::{
    app::ApprovalKind,
    domain::chat::Image,
    infra::desktop::{self, Button},
};

/// Lets the screen settle (menus, animations) before the screenshot that follows an action.
const SETTLE: Duration = Duration::from_millis(600);

fn int(args: &Value, key: &str) -> Result<i32> {
    let v = args[key].as_i64().ok_or_else(|| anyhow::anyhow!("`{key}` (entier) requis"))?;
    Ok(i32::try_from(v)?)
}

/// Runs one action, then returns a fresh screenshot so the model sees the effect.
pub async fn run(ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput> {
    ensure!(ctx.env.settings.computer, "contrôle du bureau désactivé (Paramètres → Agent)");
    let action = args["action"].as_str().unwrap_or("screenshot").to_owned();
    // Keys and clicks reach any program (Win+R, a terminal): after web content they would bypass the shell guard.
    if action != "screenshot" && ctx.reads.untrusted() {
        let reason = "contrôle du bureau après lecture de contenu web (injection possible)".to_owned();
        ensure!(super::approve(ctx, ApprovalKind::Command, format!("computer {args}"), reason).await, "action refused by the user");
    }
    let args = args.clone();
    let done = tokio::task::spawn_blocking(move || act(&action, &args)).await??;
    if done != "screenshot" {
        tokio::time::sleep(SETTLE).await;
    }
    let (png, (w, h)) = tokio::task::spawn_blocking(desktop::screenshot).await??;
    let text = format!("{done}\nécran {w}×{h} ; coordonnées = pixels de la capture jointe");
    Ok(ToolOutput { images: vec![Image::new("image/png", STANDARD.encode(png))], ..ToolOutput::from(text) })
}

fn act(action: &str, args: &Value) -> Result<String> {
    let at =
        |args: &Value| -> Result<(i32, i32)> { Ok(desktop::to_screen(int(args, "x")?, int(args, "y")?, desktop::screen_size())) };
    Ok(match action {
        "screenshot" => "screenshot".to_owned(),
        "click" | "double_click" => {
            let (x, y) = at(args)?;
            let button = Button::parse(args["button"].as_str().unwrap_or(""))?;
            desktop::click(x, y, button, action == "double_click")?;
            format!("{action} ({x}, {y})")
        }
        "move" => {
            let (x, y) = at(args)?;
            desktop::move_to(x, y)?;
            format!("souris en ({x}, {y})")
        }
        "scroll" => {
            if args["x"].is_i64() {
                let (x, y) = at(args)?;
                desktop::move_to(x, y)?;
            }
            let clicks = int(args, "amount")?;
            desktop::scroll(clicks)?;
            format!("défilement {clicks}")
        }
        "type" => {
            let text = args["text"].as_str().unwrap_or_default();
            desktop::type_text(text)?;
            format!("{} caractères tapés", text.chars().count())
        }
        "key" => {
            let spec = args["keys"].as_str().unwrap_or_default();
            desktop::press(&desktop::chord(spec)?)?;
            format!("touches {spec}")
        }
        other => bail!("action inconnue `{other}` (screenshot, click, double_click, move, scroll, type, key)"),
    })
}
