use std::marker::PhantomData;

use http::Uri;

use crate::auth::Auth;

pub struct NeedsUri(pub(crate) ());
pub struct NeedsAuth {
    pub(crate) uri: Uri,
}
pub struct NeedsPassword {
    pub(crate) uri: Uri,
    pub(crate) username: String,
}
pub struct Ready {
    pub(crate) uri: Uri,
    pub(crate) auth: Auth,
}

pub struct ClientBuilder<ClientType, State> {
    pub(crate) state: State,
    pub(crate) phantom: PhantomData<ClientType>,
}

#[derive(thiserror::Error, Debug)]
pub enum WithEmailError {
    #[error("failed to build Uri from host portion")]
    Invalidhost(#[from] http::uri::InvalidUri),
}

impl<ClientType> ClientBuilder<ClientType, NeedsUri> {
    pub(crate) fn new() -> ClientBuilder<ClientType, NeedsUri> {
        ClientBuilder {
            state: NeedsUri(()),
            phantom: PhantomData,
        }
    }

    /// Sets the host and port from a `Uri`.
    pub fn with_uri(self, uri: Uri) -> ClientBuilder<ClientType, NeedsAuth> {
        ClientBuilder {
            state: NeedsAuth { uri },
            phantom: self.phantom,
        }
    }

    /// Sets the host and username from an email.
    pub fn with_email<S: AsRef<str>>(
        self,
        email: S,
    ) -> Result<ClientBuilder<ClientType, NeedsPassword>, WithEmailError> {
        // The `Uri` type is broken for this case. See: https://github.com/hyperium/http/issues/596
        let mut parts = email.as_ref().split('@');
        let username = parts
            .next()
            .expect("split always yields are least one part");
        let host = parts.next().unwrap_or("localhost");
        Ok(ClientBuilder {
            state: NeedsPassword {
                uri: Uri::try_from(host)?,
                username: username.to_string(),
            },
            phantom: self.phantom,
        })
    }
}

impl<ClientType> ClientBuilder<ClientType, NeedsAuth> {
    /// Sets the authentication type and credentials.
    pub fn with_auth(self, auth: Auth) -> ClientBuilder<ClientType, Ready> {
        ClientBuilder {
            state: Ready {
                uri: self.state.uri,
                auth,
            },
            phantom: self.phantom,
        }
    }
}

impl<ClientType> ClientBuilder<ClientType, NeedsPassword> {
    /// Sets the password.
    pub fn with_password(self, password: String) -> ClientBuilder<ClientType, Ready> {
        ClientBuilder {
            state: Ready {
                uri: self.state.uri,
                auth: Auth::Basic {
                    username: self.state.username,
                    password: Some(password),
                },
            },
            phantom: self.phantom,
        }
    }

    /// Sets no password.
    pub fn without_password(self) -> ClientBuilder<ClientType, Ready> {
        ClientBuilder {
            state: Ready {
                uri: self.state.uri,
                auth: Auth::Basic {
                    username: self.state.username,
                    password: None,
                },
            },
            phantom: self.phantom,
        }
    }
}
