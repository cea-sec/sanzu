use anyhow::Result;

/// Retrieves username from "<username>@<realm>"
pub fn get_username_from_principal(username: &str, realms: &[String]) -> Result<String> {
    for realm in realms {
        let mut realm_and_at = "@".to_owned();
        realm_and_at.push_str(realm);
        match username.strip_suffix(&realm_and_at) {
            Some(user) => return Ok(user.to_string()),
            None => continue,
        }
    }
    return Err(anyhow!("Invalid realm"));
}
