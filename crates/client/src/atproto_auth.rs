//! AT Protocol OAuth authentication via a Personal Data Server (PDS).
//!
//! Authenticates via atproto OAuth. The flow:
//!
//! 1. Discover OAuth metadata from the PDS
//! 2. Generate DPoP + signing keys (P-256), PKCE challenge
//! 3. PAR → authorize URL → open browser
//! 4. Receive callback on localhost with authorization code
//! 5. Exchange code for tokens (DPoP-bound)
//! 6. Resolve the user's profile (handle, display name, avatar) from their PDS
//! 7. Return credentials + profile

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use atproto_identity::key::{generate_key, KeyType};
use atproto_oauth::pkce;
use atproto_oauth::resources::oauth_authorization_server;
use atproto_oauth::workflow::{
    oauth_complete, oauth_init, OAuthClient, OAuthRequest, OAuthRequestState,
};
use url::Url;

/// The client_id for Crow's OAuth registration.
/// This URL must serve the client metadata JSON document.
const CLIENT_ID: &str = "https://crow-ai.dev/oauth-client-metadata.json";

/// Resolved atproto profile for the authenticated user.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AtprotoProfile {
    pub did: String,
    pub handle: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

/// The profile resolved during the most recent authentication.
static ATPROTO_PROFILE: OnceLock<AtprotoProfile> = OnceLock::new();

/// Returns the atproto profile resolved during authentication, if any.
pub fn current_profile() -> Option<&'static AtprotoProfile> {
    ATPROTO_PROFILE.get()
}

fn profile_path() -> std::path::PathBuf {
    paths::config_dir().join("crow_profile.json")
}

/// Save the profile to disk so it survives restarts.
fn save_profile(profile: &AtprotoProfile) {
    if let Ok(json) = serde_json::to_string(profile) {
        std::fs::write(profile_path(), json).ok();
    }
}

/// Load a previously saved profile from disk and populate ATPROTO_PROFILE.
/// Returns the profile if one was found and loaded.
pub fn load_saved_profile() -> Option<&'static AtprotoProfile> {
    if ATPROTO_PROFILE.get().is_some() {
        return ATPROTO_PROFILE.get();
    }
    let json = std::fs::read_to_string(profile_path()).ok()?;
    let profile: AtprotoProfile = serde_json::from_str(&json).ok()?;
    let _ = ATPROTO_PROFILE.set(profile);
    ATPROTO_PROFILE.get()
}

/// Delete the saved profile file (called on sign-out).
pub fn clear_saved_profile() {
    std::fs::remove_file(profile_path()).ok();
}

/// Result of a successful atproto OAuth flow.
pub struct AtprotoCredentials {
    pub did: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub handle: Option<String>,
}

/// Hash a DID string to a u64 for compatibility with Zed's numeric user_id.
pub fn did_to_user_id(did: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    did.hash(&mut hasher);
    hasher.finish()
}

/// Run the full atproto OAuth flow against a PDS.
///
/// Opens the user's browser for authorization and listens on a local
/// HTTP server for the callback. Returns credentials on success.
pub async fn authenticate_with_pds(pds_url: &str) -> Result<AtprotoCredentials> {
    let http_client = atproto_reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    // 1. Discover OAuth authorization server metadata
    let auth_server = oauth_authorization_server(&http_client, pds_url)
        .await
        .map_err(|e| anyhow!("OAuth discovery failed for {pds_url}: {e}"))?;

    // 2. Generate keys and PKCE
    let signing_key = generate_key(KeyType::P256Private)
        .map_err(|e| anyhow!("key generation failed: {e}"))?;
    let dpop_key = generate_key(KeyType::P256Private)
        .map_err(|e| anyhow!("DPoP key generation failed: {e}"))?;
    let (pkce_verifier, code_challenge) = pkce::generate();

    // 3. Start local callback server on a fixed port.
    // The port must match the redirect_uris in the client metadata document.
    const CALLBACK_PORT: u16 = 19836;
    let server = tiny_http::Server::http(format!("127.0.0.1:{CALLBACK_PORT}"))
        .map_err(|e| anyhow!("failed to bind callback port {CALLBACK_PORT}: {e}"))?;
    let redirect_uri = format!("http://127.0.0.1:{CALLBACK_PORT}/callback");

    // 4. Configure OAuth client
    let oauth_client = OAuthClient {
        redirect_uri: redirect_uri.clone(),
        client_id: CLIENT_ID.to_string(),
        private_signing_key_data: signing_key,
    };

    let state = uuid::Uuid::new_v4().to_string();
    let nonce = uuid::Uuid::new_v4().to_string();

    let oauth_state = OAuthRequestState {
        state: state.clone(),
        nonce: nonce.clone(),
        code_challenge,
        scope: "atproto transition:generic".to_string(),
    };

    // 5. PAR
    let par_response = oauth_init(
        &http_client,
        &oauth_client,
        &dpop_key,
        None, // login_hint
        &auth_server,
        &oauth_state,
    )
    .await
    .map_err(|e| anyhow!("PAR failed: {e}"))?;

    // 6. Build authorization URL and open browser
    let auth_url = format!(
        "{}?client_id={}&request_uri={}",
        auth_server.authorization_endpoint,
        urlencoding::encode(&oauth_client.client_id),
        urlencoding::encode(&par_response.request_uri),
    );

    // Open the URL in the default browser
    open::that(&auth_url).context("failed to open browser for authorization")?;

    eprintln!("[atproto] Waiting for OAuth callback on port {CALLBACK_PORT}...");
    eprintln!("[atproto] If the browser didn't open, visit:\n  {auth_url}");

    // 7. Wait for callback
    let (code, returned_state) = tokio::task::spawn_blocking(move || {
        for _ in 0..300 {
            // 5 minute timeout
            if let Some(req) = server.recv_timeout(Duration::from_secs(1))? {
                let path = req.url().to_string();
                let url = Url::parse(&format!("http://127.0.0.1{}", path))
                    .context("failed to parse callback URL")?;
                let params: std::collections::HashMap<String, String> =
                    url.query_pairs().into_owned().collect();

                // Respond to the browser
                let html = "<html><body><h2>Crow</h2><p>Authorization successful. You can close this tab.</p></body></html>";
                let response = tiny_http::Response::from_string(html)
                    .with_header(tiny_http::Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"text/html"[..],
                    ).unwrap());
                let _ = req.respond(response);

                if let Some(error) = params.get("error") {
                    return Err(anyhow!(
                        "OAuth error: {} - {}",
                        error,
                        params.get("error_description").unwrap_or(&String::new())
                    ));
                }

                let code = params
                    .get("code")
                    .ok_or_else(|| anyhow!("missing code in callback"))?
                    .clone();
                let returned_state = params.get("state").cloned().unwrap_or_default();
                return Ok((code, returned_state));
            }
        }
        Err(anyhow!("timed out waiting for OAuth callback"))
    })
    .await??;

    // 8. Verify state
    if returned_state != state {
        return Err(anyhow!("OAuth state mismatch — possible CSRF"));
    }

    // 9. Build OAuthRequest for token exchange
    let oauth_request = OAuthRequest {
        oauth_state: state,
        issuer: auth_server.issuer.clone(),
        authorization_server: pds_url.to_string(),
        nonce,
        pkce_verifier,
        signing_public_key: String::new(),
        dpop_private_key: String::new(),
        created_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
    };

    // 10. Exchange code for tokens
    let token_response = oauth_complete(
        &http_client,
        &oauth_client,
        &dpop_key,
        &code,
        &oauth_request,
        &auth_server,
    )
    .await
    .map_err(|e| anyhow!("token exchange failed: {e}"))?;

    let did = token_response
        .sub
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    eprintln!("[atproto] Authenticated as {did}");

    // 11. Resolve profile (handle, display name, avatar) from the PDS
    let profile = resolve_profile(&http_client, &did, pds_url).await;
    let handle = profile.as_ref().map(|p| p.handle.clone());
    if let Some(profile) = profile {
        eprintln!("[atproto] Resolved profile: @{}", profile.handle);
        save_profile(&profile);
        let _ = ATPROTO_PROFILE.set(profile);
    }

    Ok(AtprotoCredentials {
        did,
        access_token: token_response.access_token,
        refresh_token: token_response.refresh_token,
        handle,
    })
}

/// Resolve the user's atproto profile from their DID.
///
/// Fetches the DID document (for handle + PDS endpoint), then the
/// `app.bsky.actor.profile` record (for display name + avatar).
async fn resolve_profile(
    http: &atproto_reqwest::Client,
    did: &str,
    pds_url: &str,
) -> Option<AtprotoProfile> {
    // 1. Resolve DID document
    let did_doc_url = if did.starts_with("did:plc:") {
        format!("https://plc.directory/{did}")
    } else if did.starts_with("did:web:") {
        let domain = did.strip_prefix("did:web:")?;
        format!("https://{domain}/.well-known/did.json")
    } else {
        return None;
    };

    let doc: serde_json::Value = http
        .get(&did_doc_url)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    // 2. Extract handle from alsoKnownAs ("at://handle.bsky.social")
    let handle = doc["alsoKnownAs"]
        .as_array()?
        .iter()
        .find_map(|v| v.as_str()?.strip_prefix("at://").map(String::from))
        .unwrap_or_else(|| "unknown".to_string());

    // 3. Find the PDS endpoint from the DID document services
    let doc_pds = doc["service"]
        .as_array()
        .and_then(|services| {
            services.iter().find(|s| {
                s["type"].as_str() == Some("AtprotoPersonalDataServer")
            })
        })
        .and_then(|s| s["serviceEndpoint"].as_str())
        .unwrap_or(pds_url);

    // 4. Fetch the profile record
    let record_url = format!(
        "{doc_pds}/xrpc/com.atproto.repo.getRecord?repo={}&collection=app.bsky.actor.profile&rkey=self",
        urlencoding::encode(did),
    );

    let record: serde_json::Value = http
        .get(&record_url)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    let value = &record["value"];
    let display_name = value["displayName"].as_str().map(String::from);

    // 5. Construct avatar URL from blob CID
    let avatar_url = value["avatar"]["ref"]["$link"]
        .as_str()
        .map(|cid| {
            format!(
                "{doc_pds}/xrpc/com.atproto.sync.getBlob?did={}&cid={cid}",
                urlencoding::encode(did),
            )
        });

    Some(AtprotoProfile {
        did: did.to_string(),
        handle,
        display_name,
        avatar_url,
    })
}
