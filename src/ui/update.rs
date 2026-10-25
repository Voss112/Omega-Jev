//! Silent updates: one check at start then every 6 h; the verified MSI is installed when Omega closes.

use std::time::Duration;

use super::App;
use crate::{
    app::Event,
    infra::{paths, update},
};

const EVERY: Duration = Duration::from_secs(6 * 3600);

impl App {
    /// Background loop polling the GitHub releases while auto-update is on (one loop at most, on the current
    /// HTTP client: a proxy set later is honoured, and turning the option off stops the polling).
    pub(super) fn start_updates(&mut self) {
        if let Some(task) = self.update_task.take() {
            task.abort();
        }
        if !self.settings.auto_update {
            return;
        }
        let (http, emit) = (self.env.http.clone(), self.emit.clone());
        self.update_task = Some(self.spawn(async move {
            loop {
                match update::fetch(&http, env!("CARGO_PKG_VERSION"), &paths::updates()).await {
                    Ok(Some(msi)) => {
                        emit.send(Event::UpdateReady(msi));
                        return;
                    }
                    Ok(None) => {}
                    Err(e) => eprintln!("mise à jour : {e:#}"),
                }
                tokio::time::sleep(EVERY).await;
            }
        }));
    }

    /// On exit: hands the downloaded MSI to msiexec (it replaces the files once Omega is gone).
    pub(super) fn apply_update(&mut self) {
        if let Some(msi) = self.update.take().filter(|_| self.settings.auto_update)
            && let Err(e) = update::install(&msi)
        {
            eprintln!("mise à jour : {e:#}");
        }
    }
}
