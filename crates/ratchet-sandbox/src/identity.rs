//! "Who am I" for the app's own AWS credentials. (TS `identity.ts`)

use crate::error::{Error, Result, sdk};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallerIdentity {
    pub account_id: String,
    pub arn: String,
}

#[derive(Default)]
pub struct CheckAwsCredentialsOptions {
    pub region: String,
    /// Named profile in `~/.aws/config` (static keys, SSO or assume-role). Default credential chain when unset.
    pub profile: Option<String>,
    /// The account the sandbox must live in; a mismatch is an error.
    pub expected_account_id: Option<String>,
    /// Injected in tests.
    pub client: Option<aws_sdk_sts::Client>,
}

/// STS GetCallerIdentity: proves the profile resolves, an SSO session has not expired, and optionally that it is the
/// dedicated sandbox account. Read-only.
pub async fn check_aws_credentials(opts: CheckAwsCredentialsOptions) -> Result<CallerIdentity> {
    let client = match opts.client {
        Some(c) => c,
        None => {
            let cfg =
                crate::compute::aws::load_sdk_config(&opts.region, opts.profile.as_deref()).await;
            aws_sdk_sts::Client::new(&cfg)
        }
    };
    let r = client.get_caller_identity().send().await.map_err(sdk)?;
    let account_id = r.account().unwrap_or("").to_string();
    if account_id.is_empty() {
        return Err(Error::other("STS returned no account id"));
    }
    if let Some(expected) = opts
        .expected_account_id
        .as_deref()
        .filter(|e| !e.is_empty())
        && account_id != expected
    {
        return Err(Error::other(format!(
            "credentials are for account {account_id}, expected sandbox account {expected}"
        )));
    }
    Ok(CallerIdentity {
        account_id,
        arn: r.arn().unwrap_or("").to_string(),
    })
}
