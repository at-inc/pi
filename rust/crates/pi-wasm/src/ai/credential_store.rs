//! Exports of `auth/credential-store`.

use std::rc::Rc;

use wasm_bindgen::prelude::*;

use super::auth::CredentialStoreHandle;

/// `new InMemoryCredentialStore()`.
#[wasm_bindgen(js_name = aiInMemoryCredentialStore)]
pub fn in_memory_credential_store() -> CredentialStoreHandle {
	CredentialStoreHandle::new(Rc::new(pi_ai::auth::credential_store::InMemoryCredentialStore::new()))
}
