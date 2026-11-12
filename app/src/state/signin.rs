//! The guided Apple sign-in (the onboarding's Sign in step), state side: the sign-in window, the
//! account's storefront (saved in settings, shown read-only in Settings → Account) and restarting
//! a bridge that failed. The steps themselves are `ui::onboarding`'s.

use serde_json::{Value, json};

use super::*;
use crate::settings::Storefront;

/// Apple's storefront list entry: `data[0].id` and `data[0].attributes.name`.
fn parse_storefront(v: &Value) -> Option<(String, Option<String>)> {
    let d = v.get(0).or_else(|| v.get("data").and_then(|d| d.get(0)))?;
    let id = d["id"].as_str().filter(|s| !s.is_empty())?.to_string();
    Some((id, d["attributes"]["name"].as_str().map(String::from)))
}

#[cfg(test)]
pub(super) fn parse_storefront_for_tests(v: &Value) -> Option<(String, Option<String>)> {
    parse_storefront(v)
}

impl State {
    /// Opens the sign-in window for the guided step: small and centred on `near` (our window's
    /// frame on screen, macOS) or on the screen. Again while it shows: brings it to the front.
    pub fn sign_in_window(&mut self, near: Option<[f32; 4]>) {
        self.session.login_window = true;
        #[cfg(test)]
        self.sent.borrow_mut().push(format!("signIn {near:?}"));
        if let Some(b) = &self.bridge {
            b.sign_in(near);
        }
    }

    /// A bridge that failed (Widevine, Electron gone, couldn't start): a new one.
    pub fn restart_bridge(&mut self) {
        if let Some(b) = self.bridge.take() {
            match &self.engine {
                Some(engine) => engine.stop(b),
                None => drop(b),
            }
        }
        self.session.bridge_up = false;
        self.session.error = None;
        self.session.login_window = false;
        self.start_bridge();
    }

    /// The storefront the bridge reported (`ready`, `auth`, `storefront`). The page's MusicKit
    /// uses it either way; signed in, it is the account's and is saved. Signed in with none
    /// known: Apple is asked once (`/v1/me/storefront`).
    pub(super) fn storefront_seen(&mut self, id: Option<String>) {
        match id {
            Some(id) => {
                self.session.storefront = Some(id.clone());
                if self.session.authorized {
                    self.save_storefront(id, None);
                }
            }
            None if self.session.authorized && self.settings.storefront.is_none() && !self.session.storefront_asked => {
                self.session.storefront_asked = true;
                self.request(("/v1/me/storefront".into(), json!({})), |v| parse_storefront(v), |r| {
                    Msg::Storefront(r.and_then(|f| f.ok_or_else(|| "no storefront in the reply".to_string())))
                });
            }
            None => {}
        }
    }

    /// Saves the account's storefront, and asks Apple for its name ("Italy") once if unknown.
    fn save_storefront(&mut self, id: String, name: Option<String>) {
        let known = self.settings.storefront.as_ref().filter(|f| f.id == id);
        let name = name.or_else(|| known.and_then(|f| f.name.clone()));
        let new = Storefront { id: id.clone(), name };
        if self.settings.storefront.as_ref() != Some(&new) {
            self.settings.storefront = Some(new.clone());
            self.save_settings();
        }
        if new.name.is_none() && self.session.storefront_named.as_deref() != Some(id.as_str()) {
            self.session.storefront_named = Some(id.clone());
            self.request((format!("/v1/storefronts/{id}"), json!({})), |v| parse_storefront(v), |r| {
                Msg::Storefront(r.and_then(|f| f.ok_or_else(|| "no storefront in the reply".to_string())))
            });
        }
    }

    /// Apple's answer to a storefront lookup (the fallback, or a name).
    pub(super) fn storefront_reply(&mut self, result: Result<(String, Option<String>), String>) -> bool {
        match result {
            Ok((id, name)) if self.session.authorized => {
                self.session.storefront = Some(id.clone());
                self.save_storefront(id, name);
                true
            }
            Ok(_) => false, // signed out meanwhile
            Err(e) => {
                eprintln!("uzumaki: storefront lookup: {e}");
                false
            }
        }
    }

    /// Signed out: the account's storefront goes with it.
    pub(super) fn forget_storefront(&mut self) {
        self.session.storefront_asked = false;
        self.session.storefront_named = None;
        if self.settings.storefront.take().is_some() {
            self.save_settings();
        }
    }
}
