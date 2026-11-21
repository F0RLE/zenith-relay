#[path = "support/customer_flow.rs"]
mod flow;
#[path = "support/payment_fixture.rs"]
mod payment;
mod support;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL, built web assets and Chromium"]
async fn owner_approval_identity_wallet_and_revocation() {
    flow::journey(false).await;
}
