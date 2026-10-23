//! Repository registration and runner OAuth identity; never job credentials.
use super::COMPATIBLE_RUNNER_VERSION;
use crate::{
    config::{Config, runner_arch, runner_os},
    error::{Failure, Result},
    http::*,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use rand::rngs::OsRng;
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use tokio::time::Instant;
use url::Url;
use uuid::Uuid;

pub(crate) struct Identity {
    pub(crate) key: RsaPrivateKey,
}

impl Identity {
    pub fn generate() -> Result<Self> {
        Ok(Self {
            key: RsaPrivateKey::new(&mut OsRng, 2048)?,
        })
    }

    pub fn public_xml(&self) -> String {
        format!(
            "<RSAKeyValue><Modulus>{}</Modulus><Exponent>{}</Exponent></RSAKeyValue>",
            STANDARD.encode(self.key.n().to_bytes_be()),
            STANDARD.encode(self.key.e().to_bytes_be())
        )
    }

    pub fn assertion(&self, client_id: &str, audience: &str, now: u64) -> Result<String> {
        let issued = now.saturating_sub(30);
        let claims = json!({"iss":client_id,"sub":client_id,"aud":audience,
            "nbf":issued,"iat":issued,"exp":issued.saturating_add(300),"jti":Uuid::new_v4()});
        let der = self.key.to_pkcs1_der()?;
        Ok(jsonwebtoken::encode(
            &Header::new(Algorithm::PS256),
            &claims,
            &EncodingKey::from_rsa_der(der.as_bytes()),
        )?)
    }

    #[cfg(test)]
    pub fn test_identity() -> Self {
        use rand::SeedableRng;
        static KEY: std::sync::OnceLock<RsaPrivateKey> = std::sync::OnceLock::new();
        Self {
            key: KEY
                .get_or_init(|| {
                    RsaPrivateKey::new(&mut rand::rngs::StdRng::seed_from_u64(71), 2048)
                        .expect("test key")
                })
                .clone(),
        }
    }
}

pub(crate) struct RegisteredRunner {
    pub id: u64,
    pub authorization: Result<RunnerAuthorization>,
}

#[derive(Deserialize)]
pub(crate) struct RunnerAuthorization {
    pub client_id: String,
    pub authorization_url: String,
    pub server_url: String,
}

#[derive(Deserialize)]
struct RegistrationBootstrap {
    #[serde(default)]
    use_v2_flow: bool,
    token: String,
    url: String,
    token_schema: String,
}

pub(crate) async fn register_runner(
    http: &Http,
    api: &Url,
    config: &Config,
    identity: &Identity,
) -> Result<RegisteredRunner> {
    #[derive(Deserialize)]
    struct Token {
        token: String,
    }
    let token: Token = http
        .json(
            http.client
                .post(endpoint(
                    api,
                    &format!(
                        "repos/{}/actions/runners/registration-token",
                        config.repository
                    ),
                )?)
                .header("authorization", authorization("Bearer", &config.token)?)
                .header("X-GitHub-Api-Version", "2022-11-28"),
            "obtain registration token",
        )
        .await?;
    let tenant: RegistrationBootstrap = http
        .json(
            http.client
                .post(endpoint(api, "actions/runner-registration")?)
                .header("authorization", authorization("RemoteAuth", &token.token)?)
                .json(&json!({"url":config.github_url(),"runner_event":"register"})),
            "discover registration protocol",
        )
        .await?;
    if tenant.token_schema != "OAuthAccessToken" {
        return Err(Failure::Protocol(
            "unsupported registration credential scheme",
        ));
    }
    if !tenant.use_v2_flow {
        return register_through_tenant(http, config, identity, tenant).await;
    }
    #[derive(Deserialize)]
    struct Group {
        id: u64,
        #[serde(default)]
        default: bool,
    }
    #[derive(Deserialize)]
    struct Groups {
        runner_groups: Vec<Group>,
    }
    let groups: Groups = http
        .json(
            http.client
                .get(endpoint(
                    api,
                    &format!("repos/{}/actions/runner-groups", config.repository),
                )?)
                .header("authorization", authorization("RemoteAuth", &token.token)?),
            "discover runner group",
        )
        .await?;
    let group = groups
        .runner_groups
        .iter()
        .find(|g| g.default)
        .ok_or(Failure::Protocol("no default runner group"))?;
    let labels = labels(config);
    let response: serde_json::Value = http
        .json(
            http.client
                .post(endpoint(api, "actions/runners/register")?)
                .header("authorization", authorization("RemoteAuth", &token.token)?)
                .json(
                    &json!({"url":config.github_url(),"group_id":group.id,"name":config.name,
            "version":COMPATIBLE_RUNNER_VERSION,"updates_disabled":true,"ephemeral":false,
            "labels":labels,"public_key":identity.public_xml()}),
                ),
            "register runner",
        )
        .await?;
    let id = registration_id(&response)?;
    let authorization = response
        .get("authorization")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .ok_or(Failure::Protocol("malformed runner authorization"));
    Ok(RegisteredRunner { id, authorization })
}

fn labels(config: &Config) -> Vec<serde_json::Value> {
    let mut labels = vec![
        json!({"name":"self-hosted","type":0}),
        json!({"name":runner_os(),"type":0}),
        json!({"name":runner_arch(),"type":0}),
    ];
    labels.extend(
        config
            .labels
            .iter()
            .map(|name| json!({"name":name,"type":1})),
    );
    labels
}

// GitHub.com still selects this bootstrap for some repositories. Only registration
// uses the tenant API; the returned ServerUrlV2 must lead to the modern broker.
async fn register_through_tenant(
    http: &Http,
    config: &Config,
    identity: &Identity,
    tenant: RegistrationBootstrap,
) -> Result<RegisteredRunner> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Pool {
        id: u64,
        is_internal: bool,
        is_hosted: bool,
    }
    #[derive(Deserialize)]
    struct Pools {
        value: Vec<Pool>,
    }
    let base = service_url(&tenant.url)?;
    let pools: Pools = http
        .json(
            http.client
                .get(endpoint(&base, "_apis/distributedtask/pools")?)
                .query(&[("poolType", "automation"), ("api-version", "6.0-preview")])
                .header("authorization", authorization("Bearer", &tenant.token)?),
            "discover tenant runner pool",
        )
        .await?;
    let pool = pools
        .value
        .iter()
        .find(|p| p.is_internal && !p.is_hosted)
        .ok_or(Failure::Protocol("no default runner pool"))?;
    let agent: serde_json::Value = http.json(http.client.post(endpoint(&base,&format!("_apis/distributedtask/pools/{}/agents",pool.id))?)
        .query(&[("api-version","6.0-preview")])
        .header("authorization",authorization("Bearer",&tenant.token)?)
        .json(&json!({"name":config.name,"version":COMPATIBLE_RUNNER_VERSION,
            "osDescription":format!("{} {}",runner_os(),runner_arch()),"maxParallelism":1,
            "enabled":true,"ephemeral":false,"disableUpdate":true,"labels":labels(config),
            "authorization":{"publicKey":{"exponent":STANDARD.encode(identity.key.e().to_bytes_be()),
                "modulus":STANDARD.encode(identity.key.n().to_bytes_be())}}})), "register tenant runner").await?;
    let id = registration_id(&agent)?;
    // Ancillary decoding errors travel with the identity, so the caller can
    // establish its cleanup scope before propagating them.
    let authorization = (|| {
        #[derive(Deserialize)]
        struct Property {
            #[serde(rename = "$value")]
            value: String,
        }
        #[derive(Deserialize, Default)]
        struct Properties {
            #[serde(rename = "ServerUrlV2")]
            broker: Option<Property>,
            #[serde(rename = "AuthorizationUrlV2")]
            authorization: Option<Property>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Authorization {
            client_id: String,
            authorization_url: String,
        }
        #[derive(Deserialize)]
        struct Metadata {
            authorization: Authorization,
            #[serde(default)]
            properties: Properties,
        }
        let metadata: Metadata = serde_json::from_value(agent)
            .map_err(|_| Failure::Protocol("malformed tenant runner authorization"))?;
        Ok(RunnerAuthorization {
            client_id: metadata.authorization.client_id,
            authorization_url: metadata
                .properties
                .authorization
                .map(|p| p.value)
                .unwrap_or(metadata.authorization.authorization_url),
            server_url: metadata
                .properties
                .broker
                .ok_or(Failure::Protocol("missing modern broker URL"))?
                .value,
        })
    })();
    Ok(RegisteredRunner { id, authorization })
}

fn registration_id(value: &serde_json::Value) -> Result<u64> {
    value
        .get("id")
        .and_then(serde_json::Value::as_u64)
        .filter(|id| *id > 0)
        .ok_or(Failure::Protocol(
            "registration response omitted runner identity; cleanup could not be confirmed",
        ))
}

pub(crate) async fn remove_runner(http: &Http, api: &Url, config: &Config, id: u64) -> Result<()> {
    match http
        .send(
            http.client
                .delete(endpoint(
                    api,
                    &format!("repos/{}/actions/runners/{id}", config.repository),
                )?)
                .header("authorization", authorization("Bearer", &config.token)?),
            "remove runner",
        )
        .await
    {
        Ok(_) => Ok(()),
        Err(e) if e.is_status(404) => Ok(()),
        Err(e) => Err(e),
    }
}

pub(crate) struct RunnerAuth {
    identity: Identity,
    client_id: String,
    url: Url,
    cached: Option<(String, Instant)>,
}

impl RunnerAuth {
    pub fn new(identity: Identity, auth: &RunnerAuthorization) -> Result<Self> {
        if auth.client_id.is_empty() {
            return Err(Failure::Protocol("missing runner authorization identity"));
        }
        Ok(Self {
            identity,
            client_id: auth.client_id.clone(),
            url: service_url(&auth.authorization_url)?,
            cached: None,
        })
    }

    pub fn invalidate(&mut self) {
        self.cached = None;
    }

    pub async fn token(&mut self, http: &Http) -> Result<&str> {
        if self
            .cached
            .as_ref()
            .is_none_or(|(_, expires)| Instant::now() >= *expires)
        {
            let now = chrono::Utc::now()
                .timestamp()
                .try_into()
                .map_err(|_| Failure::Protocol("invalid system clock"))?;
            let assertion = self
                .identity
                .assertion(&self.client_id, self.url.as_str(), now)?;
            #[derive(Deserialize)]
            struct Token {
                access_token: String,
                expires_in: u64,
            }
            let token: Token = http
                .json(
                    http.client.post(self.url.clone()).form(&[
                        ("grant_type", "client_credentials"),
                        (
                            "client_assertion_type",
                            "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
                        ),
                        ("client_assertion", assertion.as_str()),
                    ]),
                    "exchange runner OAuth token",
                )
                .await?;
            if token.access_token.is_empty() || token.expires_in == 0 || token.expires_in > 86400 {
                return Err(Failure::Protocol("invalid runner token expiration"));
            }
            let lifetime = token.expires_in.saturating_sub(60).max(1);
            self.cached = Some((
                token.access_token,
                Instant::now() + Duration::from_secs(lifetime),
            ));
        }
        self.cached
            .as_ref()
            .map(|(token, _)| token.as_str())
            .ok_or(Failure::Protocol("missing runner credential"))
    }
}
