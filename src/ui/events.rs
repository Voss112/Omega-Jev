//! Applies runtime events to UI state.

use std::time::Instant;

use super::App;
use crate::app::Event;

impl App {
    pub(super) fn pump(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Routed { conv, route, preamble, tools_added, toolset, thinking_from, prefix } => {
                    if let Some(c) = self.conv_mut(conv) {
                        c.toolset = toolset;
                        c.thinking_from = thinking_from;
                        c.prefix = prefix;
                        if let Some(m) = c.last_user_mut() {
                            m.route = Some(route);
                            m.preamble = preamble;
                            m.tools_added = tools_added;
                        }
                    }
                }
                Event::Delta { conv, text, reasoning } => {
                    if let Some(l) = self.live.get_mut(&conv) {
                        if reasoning { l.reasoning.push_str(&text) } else { l.text.push_str(&text) }
                    }
                }
                Event::Message { conv, msg } => {
                    // Late events from a stopped turn are dropped: the partial answer was already kept.
                    let Some(l) = self.live.get_mut(&conv) else { continue };
                    l.text.clear();
                    l.reasoning.clear();
                    if let Some(c) = self.conv_mut(conv) {
                        c.push(msg);
                    }
                }
                Event::Usage { conv, usage } => {
                    if let Some(c) = self.conv_mut(conv) {
                        c.usage += usage;
                    }
                }
                Event::Measured { conv, sample, thinking_from } => {
                    if let Some(c) = self.conv_mut(conv) {
                        c.savings.record(&sample);
                        c.thinking_from = thinking_from;
                    }
                }
                Event::Context { conv, tokens } => {
                    if let Some(c) = self.conv_mut(conv) {
                        c.context = tokens;
                    }
                }
                Event::Approval(a) => {
                    if self.live.contains_key(&a.conv) {
                        self.approvals.push(a);
                    }
                }
                Event::Checkpointed { conv, turn } => {
                    if let Some(m) = self.conv_mut(conv).and_then(|c| c.messages.iter_mut().find(|m| m.turn_key() == turn)) {
                        m.checkpoint = true;
                    }
                }
                Event::Compacted { conv, summary } => {
                    if let Some(c) = self.conv_mut(conv) {
                        c.summary = Some(summary);
                        c.context = 0;
                    }
                }
                Event::Cleared { conv, indices } => {
                    if let Some(c) = self.conv_mut(conv) {
                        crate::app::clear::apply(c, &indices);
                    }
                }
                Event::Done { conv, error } => {
                    if self.live.remove(&conv).is_none() {
                        continue;
                    }
                    if let Some(e) = error {
                        self.errors.insert(conv, e);
                    }
                    self.save(conv);
                    // The turn may have written files: the tree must re-read the disk.
                    self.files.invalidate();
                }
                Event::Health(h) => self.health = h,
                Event::Bench(rows) => {
                    self.bench = Some(rows);
                    self.bench_running = false;
                }
                Event::Vpn(v) => self.vpn = Some(v),
                Event::Connected { provider, result } => self.connected(provider, result),
                Event::PlanUsage { provider, windows } => {
                    self.plan_usage.insert(provider, (windows, Instant::now()));
                }
                Event::DeviceCode { code, url } => {
                    if let Some(login) = self.login.as_mut().filter(|l| l.provider == crate::domain::auth::Provider::GitHub) {
                        login.user_code = code;
                        login.url = url;
                        login.status = "En attente de la validation sur github.com…".to_owned();
                    }
                }
                Event::Models { engine, result } => match result {
                    Ok(models) => {
                        self.catalog.insert(engine, models);
                        self.catalog_error.remove(&engine);
                    }
                    Err(e) => {
                        self.catalog_error.insert(engine, e);
                    }
                },
            }
        }
    }
}
