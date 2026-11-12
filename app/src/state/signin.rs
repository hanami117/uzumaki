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

/// What Apple's `/v1/me/storefront` answer means: the account's storefront `id`, the value to
/// save when the saved one differs (a saved name is kept for the same id), and whether the page's
/// MusicKit (on `page`) must switch to it. A failed lookup is passed on: keep everything.
#[derive(Debug, PartialEq)]
pub(super) struct AccountStorefront {
    pub id: String,
    pub save: Option<Storefront>,
    pub switch_page: bool,
}

pub(super) fn account_storefront(
    saved: Option<&Storefront>,
    page: Option<&str>,
    answer: Result<(String, Option<String>), String>,
) -> Result<AccountStorefront, String> {
    let (id, name) = answer?;
    let id = id.to_lowercase();
    let name = name.or_else(|| saved.filter(|f| f.id == id).and_then(|f| f.name.clone()));
    let new = Storefront { id: id.clone(), name };
    let save = (saved != Some(&new)).then_some(new);
    Ok(AccountStorefront { switch_page: page != Some(id.as_str()), id, save })
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

    /// The storefront the bridge reported (`ready`, `auth`, `storefront`): the page's MusicKit's
    /// (`storefrontId`). The page starts on its URL's ("us") and keeps it after a sign-in, so it
    /// is never taken as the account's. Signed in, Apple is asked once per start or sign-in
    /// (`/v1/me/storefront`), and until it answers the saved storefront (an earlier answer) is
    /// put on the page, so catalog requests use it from the start.
    pub(super) fn storefront_seen(&mut self, id: Option<String>) {
        if let Some(id) = id {
            self.session.storefront = Some(id);
        }
        if !self.session.authorized {
            return;
        }
        let want = self.session.account_storefront.clone().or_else(|| self.settings.storefront.as_ref().map(|f| f.id.clone()));
        if let Some(want) = want.filter(|w| self.session.storefront.as_deref() != Some(w.as_str())) {
            self.switch_page_storefront(want);
        }
        if !self.session.storefront_asked {
            self.session.storefront_asked = true;
            self.request(("/v1/me/storefront".into(), json!({})), |v| parse_storefront(v), |r| {
                Msg::AccountStorefront(r.and_then(|f| f.ok_or_else(|| "no storefront in the reply".to_string())))
            });
        }
    }

    /// Apple's answer to `/v1/me/storefront`: the account's storefront, saved and put on the page.
    /// A failed lookup keeps what there is (logged; asked again at the next start).
    pub(super) fn account_storefront_reply(&mut self, result: Result<(String, Option<String>), String>) -> bool {
        if !self.session.authorized {
            return false; // signed out meanwhile
        }
        let fix = match account_storefront(self.settings.storefront.as_ref(), self.session.storefront.as_deref(), result) {
            Ok(fix) => fix,
            Err(e) => {
                eprintln!("uzumaki: storefront: account lookup failed ({e}), keeping {:?}", self.session.storefront);
                return false;
            }
        };
        eprintln!(
            "uzumaki: storefront: account {}, page {}{}",
            fix.id,
            self.session.storefront.as_deref().unwrap_or("?"),
            if fix.switch_page { " -> switching the page" } else { "" }
        );
        self.session.account_storefront = Some(fix.id.clone());
        if let Some(new) = fix.save {
            self.settings.storefront = Some(new);
            self.save_storefront_name();
            self.save_settings();
        }
        if fix.switch_page {
            self.switch_page_storefront(fix.id);
        }
        true
    }

    /// Puts the page's MusicKit on `id` (catalog requests and playback follow).
    fn switch_page_storefront(&mut self, id: String) {
        #[cfg(test)]
        self.sent.borrow_mut().push(format!("setStorefront {id}"));
        if let Some(b) = &self.bridge {
            let asked = id.clone();
            b.set_storefront(&id, move |r| {
                match r {
                    Ok(v) => eprintln!("uzumaki: storefront: page {} -> {} (account {})", v["before"], v["storefront"], v["account"]),
                    Err(e) => eprintln!("uzumaki: storefront: setting the page to {asked} failed: {e}"),
                }
                None
            });
        }
        self.session.storefront = Some(id);
        // Pages already asked for came from the other store's catalog.
        self.reload_catalog();
    }

    /// Drops the catalog pages asked for this session (browse, radio, artists, search) and loads
    /// what the current page needs again. Nothing asked yet (the usual start: the page switches
    /// before anything loads): nothing to do.
    fn reload_catalog(&mut self) {
        let asked = self.browse_new != Load::Idle
            || self.made_for_you != Load::Idle
            || self.radio.iter().any(|r| *r != Load::Idle)
            || !self.artist_pages.is_empty()
            || !self.artist_albums.is_empty()
            || self.search != Load::Idle;
        if !asked {
            return;
        }
        self.browse_new = Load::Idle;
        self.made_for_you = Load::Idle;
        self.radio = Default::default();
        self.artist_pages.clear();
        self.artist_albums.clear();
        if self.search != Load::Idle && !self.search_query.is_empty() {
            let q = self.search_query.clone();
            self.search(&q);
        }
        self.load_route_data();
    }

    /// Asks Apple for the saved storefront's name ("Italy") once, if it has none.
    fn save_storefront_name(&mut self) {
        let Some(f) = self.settings.storefront.as_ref().filter(|f| f.name.is_none()) else { return };
        if self.session.storefront_named.as_deref() == Some(f.id.as_str()) {
            return;
        }
        let id = f.id.clone();
        self.session.storefront_named = Some(id.clone());
        self.request((format!("/v1/storefronts/{id}"), json!({})), |v| parse_storefront(v), |r| {
            Msg::Storefront(r.and_then(|f| f.ok_or_else(|| "no storefront in the reply".to_string())))
        });
    }

    /// Apple's answer to a name lookup: the name goes with the saved storefront if it is the same.
    pub(super) fn storefront_reply(&mut self, result: Result<(String, Option<String>), String>) -> bool {
        match result {
            Ok((id, Some(name))) if self.session.authorized => {
                let Some(f) = self.settings.storefront.as_mut().filter(|f| f.id == id && f.name.as_deref() != Some(name.as_str())) else {
                    return false;
                };
                f.name = Some(name);
                self.save_settings();
                true
            }
            Ok(_) => false, // signed out meanwhile, another storefront by now, or no name
            Err(e) => {
                eprintln!("uzumaki: storefront name lookup: {e}");
                false
            }
        }
    }

    /// Signed out: the account's storefront goes with it.
    pub(super) fn forget_storefront(&mut self) {
        self.session.storefront_asked = false;
        self.session.account_storefront = None;
        self.session.storefront_named = None;
        if self.settings.storefront.take().is_some() {
            self.save_settings();
        }
    }
}
