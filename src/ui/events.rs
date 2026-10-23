//! Applies runtime events to UI state.

use std::time::Instant;

use super::App;
use crate::app::Event;

impl App {
    pub(super) fn pump(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Routed { conv, route, preamble, toolset } => {
                    if let Some(c) = self.conv_mut(conv) {
                        c.toolset = toolset;
                        if let Some(m) = c.last_user_mut() {
                            m.route = Some(route);
                            m.preamble = preamble;
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
                Event::Vpn(v) => self.vpn = Some(v),
                Event::Connected { provider, result } => self.connected(provider, result),
                Event::PlanUsage { provider, windows } => self.plan_usage = Some((provider, windows, Instant::now())),
            }
        }
    }
}
