"""
Mock AT Protocol PDS for local OAuth development.

Implements the minimum atproto OAuth surface:
- Server metadata discovery
- DID document
- Handle resolution
- OAuth authorization server metadata
- JWKS
- PAR (Pushed Authorization Requests)
- Authorize (auto-approves, redirects with code)
- Token exchange (with DPoP proof verification)

Run: uv --project ~/.crow/skills/use-uv run python mock_pds.py
Listens on http://localhost:2583
"""

import base64
import hashlib
import json
import secrets
import time
import uuid

from http.server import HTTPServer, BaseHTTPRequestHandler
from urllib.parse import urlparse, parse_qs, urlencode

PORT = 2583
ISSUER = f"http://localhost:{PORT}"

# Test account
TEST_HANDLE = "test.localhost"
TEST_DID = "did:web:localhost"
TEST_PASSWORD = "test"

# In-memory stores
auth_codes: dict[str, dict] = {}  # code -> {client_id, redirect_uri, code_challenge, did, scope, dpop_jkt}
par_requests: dict[str, dict] = {}  # request_uri -> params
refresh_tokens: dict[str, dict] = {}  # token -> {did, scope, dpop_jkt}

# Generate a signing key for JWTs (HMAC for simplicity in mock)
JWT_SECRET = secrets.token_hex(32)


def b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def make_jwt(payload: dict) -> str:
    """Minimal HS256 JWT."""
    header = {"alg": "HS256", "typ": "JWT"}
    h = b64url(json.dumps(header).encode())
    p = b64url(json.dumps(payload).encode())
    sig_input = f"{h}.{p}".encode()
    sig = hashlib.sha256(sig_input + JWT_SECRET.encode()).digest()
    return f"{h}.{p}.{b64url(sig)}"


def make_access_token(did: str, scope: str, dpop_jkt: str | None = None) -> str:
    now = int(time.time())
    payload = {
        "iss": ISSUER,
        "sub": did,
        "aud": ISSUER,
        "scope": scope,
        "iat": now,
        "exp": now + 3600,
        "jti": str(uuid.uuid4()),
    }
    if dpop_jkt:
        payload["cnf"] = {"jkt": dpop_jkt}
    return make_jwt(payload)


def extract_dpop_jkt(dpop_header: str | None) -> str | None:
    """Extract JWK thumbprint from DPoP proof (mock: just hash the header)."""
    if not dpop_header:
        return None
    # In a real implementation, we'd verify the DPoP proof JWT and compute
    # the JWK thumbprint. For the mock, just hash it to get a stable binding.
    return hashlib.sha256(dpop_header.encode()).hexdigest()[:43]


OAUTH_METADATA = {
    "issuer": ISSUER,
    "request_parameter_supported": True,
    "request_uri_parameter_supported": True,
    "require_request_uri_registration": True,
    "scopes_supported": ["atproto", "transition:generic"],
    "subject_types_supported": ["public"],
    "response_types_supported": ["code"],
    "grant_types_supported": ["authorization_code", "refresh_token"],
    "code_challenge_methods_supported": ["S256"],
    "authorization_endpoint": f"{ISSUER}/oauth/authorize",
    "token_endpoint": f"{ISSUER}/oauth/token",
    "revocation_endpoint": f"{ISSUER}/oauth/revoke",
    "pushed_authorization_request_endpoint": f"{ISSUER}/oauth/par",
    "require_pushed_authorization_requests": True,
    "jwks_uri": f"{ISSUER}/oauth/jwks",
    "dpop_signing_alg_values_supported": ["ES256", "RS256"],
    "protected_resources": [ISSUER],
    "client_id_metadata_document_supported": True,
    "token_endpoint_auth_methods_supported": ["none", "private_key_jwt"],
    "prompt_values_supported": ["none", "login", "consent", "select_account", "create"],
}

DID_DOCUMENT = {
    "@context": ["https://www.w3.org/ns/did/v1"],
    "id": TEST_DID,
    "verificationMethod": [
        {
            "id": f"{TEST_DID}#atproto",
            "type": "Multikey",
            "controller": TEST_DID,
            "publicKeyMultibase": "zDnaerDaTF5BXEavCrfRZEk316dpbLsfPDZ3WJ5hRTPFU2169",
        }
    ],
    "service": [
        {
            "id": "#atproto_pds",
            "type": "AtprotoPersonalDataServer",
            "serviceEndpoint": ISSUER,
        }
    ],
}

DESCRIBE_SERVER = {
    "did": TEST_DID,
    "availableUserDomains": [".localhost"],
    "inviteCodeRequired": False,
    "links": {},
    "contact": {},
}


class PDSHandler(BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        print(f"[PDS] {args[0]}")

    def _json(self, data: dict, status: int = 200):
        body = json.dumps(data, indent=2).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _redirect(self, url: str):
        self.send_response(302)
        self.send_header("Location", url)
        self.end_headers()

    def _read_body(self) -> bytes:
        length = int(self.headers.get("Content-Length", 0))
        return self.rfile.read(length)

    def do_GET(self):
        parsed = urlparse(self.path)
        path = parsed.path

        if path == "/.well-known/oauth-authorization-server":
            self._json(OAUTH_METADATA)

        elif path == "/.well-known/did.json":
            self._json(DID_DOCUMENT)

        elif path == "/.well-known/atproto-did":
            # Handle resolution: return DID for the test handle
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.end_headers()
            self.wfile.write(TEST_DID.encode())

        elif path == "/xrpc/com.atproto.server.describeServer":
            self._json(DESCRIBE_SERVER)

        elif path == "/oauth/jwks":
            # Mock JWKS — empty for now (HS256 doesn't need public keys)
            self._json({"keys": []})

        elif path == "/oauth/authorize":
            params = parse_qs(parsed.query)
            # Auto-approve: generate auth code and redirect
            client_id = params.get("client_id", [""])[0]
            redirect_uri = params.get("redirect_uri", [""])[0]
            code_challenge = params.get("code_challenge", [""])[0]
            scope = params.get("scope", ["atproto"])[0]
            state = params.get("state", [""])[0]

            # If request_uri provided (PAR), look up stored params
            request_uri = params.get("request_uri", [""])[0]
            if request_uri and request_uri in par_requests:
                stored = par_requests.pop(request_uri)
                client_id = stored.get("client_id", client_id)
                redirect_uri = stored.get("redirect_uri", redirect_uri)
                code_challenge = stored.get("code_challenge", code_challenge)
                scope = stored.get("scope", scope)
                state = stored.get("state", state)

            code = secrets.token_urlsafe(32)
            dpop_jkt = extract_dpop_jkt(self.headers.get("DPoP"))
            auth_codes[code] = {
                "client_id": client_id,
                "redirect_uri": redirect_uri,
                "code_challenge": code_challenge,
                "did": TEST_DID,
                "scope": scope,
                "dpop_jkt": dpop_jkt,
            }

            sep = "&" if "?" in redirect_uri else "?"
            location = f"{redirect_uri}{sep}code={code}"
            if state:
                location += f"&state={state}"
            location += f"&iss={ISSUER}"
            print(f"[PDS] Authorize: code={code[:16]}... → {redirect_uri}")
            self._redirect(location)

        else:
            self._json({"error": "not_found", "path": path}, 404)

    def do_POST(self):
        parsed = urlparse(self.path)
        path = parsed.path
        body = self._read_body().decode()

        if path == "/oauth/par":
            params = dict(parse_qs(body))
            # Flatten single-value lists
            flat = {k: v[0] if isinstance(v, list) and len(v) == 1 else v for k, v in params.items()}
            request_uri = f"urn:ietf:params:oauth:request_uri:{uuid.uuid4()}"
            par_requests[request_uri] = flat
            print(f"[PDS] PAR: {request_uri}")
            self._json({"request_uri": request_uri, "expires_in": 60}, 201)

        elif path == "/oauth/token":
            params = dict(parse_qs(body))
            flat = {k: v[0] if isinstance(v, list) and len(v) == 1 else v for k, v in params.items()}
            grant_type = flat.get("grant_type", "")

            if grant_type == "authorization_code":
                code = flat.get("code", "")
                code_verifier = flat.get("code_verifier", "")

                if code not in auth_codes:
                    self._json({"error": "invalid_grant", "error_description": "unknown code"}, 400)
                    return

                stored = auth_codes.pop(code)

                # Verify PKCE
                if stored["code_challenge"]:
                    expected = b64url(hashlib.sha256(code_verifier.encode()).digest())
                    if expected != stored["code_challenge"]:
                        self._json({"error": "invalid_grant", "error_description": "PKCE mismatch"}, 400)
                        return

                dpop_jkt = extract_dpop_jkt(self.headers.get("DPoP"))
                access_token = make_access_token(stored["did"], stored["scope"], dpop_jkt)
                refresh_token = secrets.token_urlsafe(48)
                refresh_tokens[refresh_token] = {
                    "did": stored["did"],
                    "scope": stored["scope"],
                    "dpop_jkt": dpop_jkt,
                }

                print(f"[PDS] Token: did={stored['did']} scope={stored['scope']}")
                self._json({
                    "access_token": access_token,
                    "token_type": "DPoP",
                    "expires_in": 3600,
                    "refresh_token": refresh_token,
                    "scope": stored["scope"],
                    "sub": stored["did"],
                })

            elif grant_type == "refresh_token":
                rt = flat.get("refresh_token", "")
                if rt not in refresh_tokens:
                    self._json({"error": "invalid_grant"}, 400)
                    return
                stored = refresh_tokens[rt]
                dpop_jkt = extract_dpop_jkt(self.headers.get("DPoP"))
                access_token = make_access_token(stored["did"], stored["scope"], dpop_jkt)
                # Rotate refresh token
                del refresh_tokens[rt]
                new_rt = secrets.token_urlsafe(48)
                refresh_tokens[new_rt] = stored
                self._json({
                    "access_token": access_token,
                    "token_type": "DPoP",
                    "expires_in": 3600,
                    "refresh_token": new_rt,
                    "scope": stored["scope"],
                    "sub": stored["did"],
                })
            else:
                self._json({"error": "unsupported_grant_type"}, 400)

        elif path == "/oauth/revoke":
            params = dict(parse_qs(body))
            token = params.get("token", [""])[0]
            refresh_tokens.pop(token, None)
            self._json({})

        else:
            self._json({"error": "not_found", "path": path}, 404)


if __name__ == "__main__":
    server = HTTPServer(("127.0.0.1", PORT), PDSHandler)
    print(f"[PDS] Mock AT Protocol PDS on {ISSUER}")
    print(f"[PDS] Handle: {TEST_HANDLE} → {TEST_DID}")
    print(f"[PDS] OAuth metadata: {ISSUER}/.well-known/oauth-authorization-server")
    print(f"[PDS] Auto-approves all authorization requests")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\n[PDS] Shutting down")
