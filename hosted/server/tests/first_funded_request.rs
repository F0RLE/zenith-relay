#[path = "support/customer_flow.rs"]
mod flow;
#[path = "support/payment_fixture.rs"]
mod payment;
mod support;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL, built web assets and Chromium"]
async fn funded_native_request_receipt_and_crash_recovery() {
    flow::journey(true).await;
}
