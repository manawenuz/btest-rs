//! LDAP/Active Directory authentication for btest-server-pro.
//!
//! Authenticates users against an LDAP directory using simple bind.

use ldap3::{LdapConnAsync, Scope, SearchEntry};

pub struct LdapConfig {
    pub url: String,
    pub base_dn: String,
    pub bind_dn: Option<String>,
    pub bind_pass: Option<String>,
}

pub struct LdapAuth {
    config: LdapConfig,
}

/// Escape special characters in LDAP filter values per RFC 4515.
fn ldap_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '\\' => out.push_str("\\5c"),
            '*' => out.push_str("\\2a"),
            '(' => out.push_str("\\28"),
            ')' => out.push_str("\\29"),
            '\0' => out.push_str("\\00"),
            _ => out.push(c),
        }
    }
    out
}

impl LdapAuth {
    pub fn new(config: LdapConfig) -> Self {
        Self { config }
    }

    /// Authenticate a user by attempting an LDAP bind.
    /// Returns Ok(true) if authentication succeeds.
    pub async fn authenticate(&self, username: &str, password: &str) -> anyhow::Result<bool> {
        let (conn, mut ldap) = LdapConnAsync::new(&self.config.url).await?;
        ldap3::drive!(conn);

        let safe_username = ldap_escape(username);

        // If service account configured, bind first to search for user DN
        let user_dn = if let (Some(ref bind_dn), Some(ref bind_pass)) =
            (&self.config.bind_dn, &self.config.bind_pass)
        {
            let result = ldap.simple_bind(bind_dn, bind_pass).await?;
            if result.rc != 0 {
                tracing::warn!("LDAP service bind failed: rc={}", result.rc);
                return Ok(false);
            }

            // Search for the user
            let filter = format!(
                "(&(objectClass=person)(|(uid={})(sAMAccountName={})(cn={})))",
                safe_username, safe_username, safe_username
            );
            let (results, _) = ldap
                .search(&self.config.base_dn, Scope::Subtree, &filter, vec!["dn"])
                .await?
                .success()?;

            if results.is_empty() {
                tracing::debug!("LDAP user not found: {}", username);
                return Ok(false);
            }

            let entry = match results.into_iter().next() {
                Some(r) => SearchEntry::construct(r),
                None => {
                    tracing::debug!("LDAP user not found: {}", username);
                    return Ok(false);
                }
            };
            entry.dn
        } else {
            // No service account — construct DN directly
            format!("uid={},{}", safe_username, self.config.base_dn)
        };

        // Attempt user bind
        let result = ldap.simple_bind(&user_dn, password).await?;
        let success = result.rc == 0;

        if success {
            tracing::info!("LDAP auth successful for {} (dn={})", username, user_dn);
        } else {
            tracing::warn!("LDAP auth failed for {} (dn={}): rc={}", username, user_dn, result.rc);
        }

        let _ = ldap.unbind().await;
        Ok(success)
    }
}
