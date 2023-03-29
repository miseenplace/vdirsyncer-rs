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

#[allow(clippy::module_name_repetitions)]
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
    ///
    /// # Caveats
    ///
    /// - Only supports emails with up to one `@`. Realistically, this should never be a problem.
    /// - Quotes are not handled with any special treatment, and will likely misbehave.
    ///
    /// Some invalid input MAY parse as valid. This interface is experimental. This should not be a
    /// problem for normal email addresses.
    ///
    /// # Errors
    ///
    /// If building the `base_uri` fails with the host extracted from the email address.
    pub fn with_email<S: AsRef<str>>(
        self,
        email: S,
    ) -> Result<ClientBuilder<ClientType, NeedsPassword>, WithEmailError> {
        // The `Uri` type is broken for this case. See: https://github.com/hyperium/http/issues/596
        let (username, host) = split_email(email.as_ref());
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

/// Splits an email into username and host parts
/// NOTE: this method is not public, so its caveats are listed for the `with_email` function above.
fn split_email(email: &str) -> (&str, &str) {
    let mut parts = email.rsplitn(2, '@');
    let last = parts
        .next()
        .expect("split always yields are least one part");
    match parts.next() {
        Some(username) => (username, last),
        None => (last, "localhost"),
    }
}

#[cfg(test)]
mod test {
    use crate::builder::split_email;

    #[test]
    fn test_split_email() {
        assert_eq!(
            split_email("charlie@example.com"),
            ("charlie", "example.com")
        );
        assert_eq!(split_email("root"), ("root", "localhost"));
        // TODO: Should quotes be removed here?
        assert_eq!(
            split_email("\"user@something\"@example.com"),
            ("\"user@something\"", "example.com")
        );
    }
}
