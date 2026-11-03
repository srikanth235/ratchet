//! Port of `packages/aws/src/identity.test.ts`.

use crate::common::meta;
use aws_sdk_sts::operation::get_caller_identity::{
    GetCallerIdentityError, GetCallerIdentityOutput,
};
use aws_smithy_mocks::{RuleMode, mock, mock_client};
use ratchet_sandbox::identity::{
    CallerIdentity, CheckAwsCredentialsOptions, check_aws_credentials,
};

fn client(account: &'static str, arn: &'static str) -> aws_sdk_sts::Client {
    let rule = mock!(aws_sdk_sts::Client::get_caller_identity).then_output(move || {
        GetCallerIdentityOutput::builder()
            .account(account)
            .arn(arn)
            .build()
    });
    mock_client!(aws_sdk_sts, RuleMode::MatchAny, &[&rule])
}

fn opts(client: aws_sdk_sts::Client, expected: Option<&str>) -> CheckAwsCredentialsOptions {
    CheckAwsCredentialsOptions {
        region: "us-east-1".into(),
        profile: Some("sandbox".into()),
        expected_account_id: expected.map(str::to_string),
        client: Some(client),
    }
}

#[tokio::test]
async fn returns_the_caller_identity() {
    let got = check_aws_credentials(opts(
        client("123456789012", "arn:aws:sts::123456789012:assumed-role/x/y"),
        None,
    ))
    .await
    .unwrap();
    assert_eq!(
        got,
        CallerIdentity {
            account_id: "123456789012".into(),
            arn: "arn:aws:sts::123456789012:assumed-role/x/y".into()
        }
    );
}

#[tokio::test]
async fn fails_when_the_credentials_are_for_another_account() {
    let e = check_aws_credentials(opts(client("999999999999", "a"), Some("123456789012")))
        .await
        .unwrap_err();
    assert!(
        e.to_string()
            .contains("expected sandbox account 123456789012"),
        "{e}"
    );
}

#[tokio::test]
async fn surfaces_credential_errors_e_g_an_expired_sso_session() {
    let rule = mock!(aws_sdk_sts::Client::get_caller_identity).then_error(|| {
        GetCallerIdentityError::generic(meta(
            "ExpiredToken",
            "The SSO session associated with this profile has expired",
        ))
    });
    let c = mock_client!(aws_sdk_sts, RuleMode::MatchAny, &[&rule]);
    let e = check_aws_credentials(opts(c, None)).await.unwrap_err();
    assert!(e.to_string().contains("SSO session"), "{e}");
}

#[tokio::test]
async fn accepts_an_injected_client_and_refuses_an_empty_account() {
    assert_eq!(
        check_aws_credentials(opts(client("111111111111", "arn"), None))
            .await
            .unwrap()
            .account_id,
        "111111111111"
    );
    let rule = mock!(aws_sdk_sts::Client::get_caller_identity)
        .then_output(|| GetCallerIdentityOutput::builder().build());
    let c = mock_client!(aws_sdk_sts, RuleMode::MatchAny, &[&rule]);
    let e = check_aws_credentials(opts(c, None)).await.unwrap_err();
    assert!(e.to_string().contains("no account id"), "{e}");
}
