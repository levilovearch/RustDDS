use serde::{Deserialize, Serialize};
use speedy::{Readable, Writable};
use bytes::Bytes;

use crate::security::types::DataHolder;

// Some generic message class IDs for authentication (see section 7.4.3.5 of the
// Security spec)
pub const GMCLASSID_SECURITY_AUTH_REQUEST: &str = "dds.sec.auth_request";
pub const GMCLASSID_SECURITY_AUTH_HANDSHAKE: &str = "dds.sec.auth";

// ValidationOutcome is like ValidationResult_t in the the Security
// specification v.1.1 (section 8.3.2.11.1), but does not contain
// VALIDATION_FAILED. Failure is handled as an error in the result type
// SecurityResult
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationOutcome {
  Ok,
  // PendingRetry,  // Not included because builtin plugins never return this
  PendingHandshakeRequest,
  PendingHandshakeMessage,
  OkFinalMessage,
}

// Handles used by the authentication plugin: opaque local references to
// internal state within the AuthenticationPlugin (sec. 8.3.2 of the Security
// specification)
pub type IdentityHandle = u32;
pub type HandshakeHandle = u32;

// Shared secret resulting from successful handshake
pub type SharedSecret = Bytes;
pub struct SharedSecretHandle {
  pub shared_secret: SharedSecret,
  pub challenge1: Bytes,
  pub challenge2: Bytes,
}

// IdentityToken: section 8.3.2.1 of the Security specification (v. 1.1)
#[derive(Debug, Clone, PartialEq, Eq, Readable, Writable)]
pub struct IdentityToken {
  // TODO: Readable & Writable are now derived, but likely need to be implemented manually.
  // Readable and Writable are needed to (de)serialize to(from) ParameterList.
  // Note: The implementation has to observe CDR alignment rules.
  pub data_holder: DataHolder,
}

impl From<DataHolder> for IdentityToken {
  fn from(value: DataHolder) -> Self {
    Self { data_holder: value }
  }
}

impl IdentityToken {
  pub fn class_id(&self) -> String {
    self.data_holder.class_id.clone()
  }

  // Mock value used for development
  pub fn dummy() -> Self {
    Self {
      data_holder: DataHolder::dummy(),
    }
  }
}

// IdentityStatusToken: section 8.3.2.2 of the Security specification (v.
// 1.1)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Readable, Writable)]
pub struct IdentityStatusToken {
  // TODO: Readable & Writable are now derived, but likely need to be implemented manually.
  // Note: The implementation has to observe CDR alignment rules.
  pub data_holder: DataHolder,
}

impl From<DataHolder> for IdentityStatusToken {
  fn from(value: DataHolder) -> Self {
    Self { data_holder: value }
  }
}

impl IdentityStatusToken {
  // Mock value used for development
  pub fn dummy() -> Self {
    Self {
      data_holder: DataHolder::dummy(),
    }
  }
}

// AuthRequestMessageToken: section 8.3.2.5 of the Security specification
// (v. 1.1)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthRequestMessageToken {
  // TODO: Readable & Writable are now derived, but likely need to be implemented manually.
  // Readable and Writable are needed to (de)serialize to(from) ParameterList.
  // Note: The implementation has to observe CDR alignment rules.
  pub data_holder: DataHolder,
}

impl From<DataHolder> for AuthRequestMessageToken {
  fn from(value: DataHolder) -> Self {
    Self { data_holder: value }
  }
}

impl AuthRequestMessageToken {
  // Mock value used for development
  pub fn dummy() -> Self {
    Self {
      data_holder: DataHolder::dummy(),
    }
  }
}

// HandshakeMessageToken: section 8.3.2.6 of the Security specification
// (v. 1.1)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeMessageToken {
  pub data_holder: DataHolder,
}

impl From<DataHolder> for HandshakeMessageToken {
  fn from(value: DataHolder) -> Self {
    Self { data_holder: value }
  }
}

impl HandshakeMessageToken {
  // Mock value used for development
  pub fn dummy() -> Self {
    Self {
      data_holder: DataHolder::dummy(),
    }
  }
}

// AuthenticatedPeerCredentialToken: section 8.3.2.7 of the Security
// specification (v. 1.1)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthenticatedPeerCredentialToken {
  pub data_holder: DataHolder,
}

impl From<DataHolder> for AuthenticatedPeerCredentialToken {
  fn from(value: DataHolder) -> Self {
    Self { data_holder: value }
  }
}

impl AuthenticatedPeerCredentialToken {
  // Mock value used for development
  pub fn dummy() -> Self {
    Self {
      data_holder: DataHolder::dummy(),
    }
  }
}
