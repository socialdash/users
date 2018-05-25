//! Models for managing Json Web Token
use models::Provider;

/// Json Web Token created by provider user status
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum UserStatus {
    New(i32),
    Exists,
}

/// Json Web Token Model sent back to gateway created by email and password
#[derive(Clone, Debug, Serialize, Deserialize, Queryable)]
pub struct JWT {
    pub token: String,
    pub status: UserStatus,
}

/// Payload received from gateway for creating JWT token by provider
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderOauth {
    pub token: String,
}

/// Json web token payload
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JWTPayload {
    pub user_id: i32,
    pub exp: i64,
    pub provider: Provider,
}

impl JWTPayload {
    pub fn new(id: i32, exp_arg: i64, provider_arg: Provider) -> Self {
        Self { user_id: id, exp: exp_arg, provider: provider_arg }
    }
}
