use sqlx::PgPool;
use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
    },
};
use uuid::Uuid;
use zenith_relay_hosted::payments::refunds::{
    AuthorizedRefund, RefundAdapter, RefundEvidence, RefundObservation,
};

pub struct Merchant {
    pub binding: Uuid,
    pub invoice: String,
    pub lose_reply: AtomicBool,
    pub refuse: AtomicBool,
    pub wrong_total: AtomicBool,
    pub lookups: AtomicUsize,
    effects: Mutex<HashMap<Uuid, (i64, bool)>>,
}

impl Merchant {
    pub fn new(binding: Uuid, invoice: String) -> Self {
        Self {
            binding,
            invoice,
            lose_reply: false.into(),
            refuse: false.into(),
            wrong_total: false.into(),
            lookups: 0.into(),
            effects: Mutex::default(),
        }
    }

    pub fn submissions(&self) -> usize {
        self.effects.lock().unwrap().len()
    }

    fn evidence(&self, refund: &AuthorizedRefund, amount: i64, refused: bool) -> RefundObservation {
        let evidence = RefundEvidence {
            merchant_binding: self.binding,
            id: refund.id,
            invoice_reference: self.invoice.clone(),
            currency: "USD".into(),
            amount_minor: amount + i64::from(self.wrong_total.load(SeqCst)),
            reference: format!("refund-{}", refund.id),
        };
        if refused {
            RefundObservation::Failed(evidence)
        } else {
            RefundObservation::Succeeded(evidence)
        }
    }
}

impl RefundAdapter for Merchant {
    fn binding(&self) -> Uuid {
        self.binding
    }

    async fn submit(&self, refund: &AuthorizedRefund) -> RefundObservation {
        assert_eq!(
            (&*refund.invoice_reference, &*refund.currency),
            (&*self.invoice, "USD")
        );
        let refused = self.refuse.load(SeqCst);
        assert!(
            self.effects
                .lock()
                .unwrap()
                .insert(refund.id, (refund.amount_minor, refused))
                .is_none(),
            "merchant operation must never be submitted twice"
        );
        if self.lose_reply.load(SeqCst) {
            RefundObservation::Uncertain
        } else {
            self.evidence(refund, refund.amount_minor, refused)
        }
    }

    async fn lookup(&self, refund: &AuthorizedRefund) -> RefundObservation {
        self.lookups.fetch_add(1, SeqCst);
        match self.effects.lock().unwrap().get(&refund.id).copied() {
            Some((amount, refused)) => self.evidence(refund, amount, refused),
            None => RefundObservation::Uncertain,
        }
    }
}

pub async fn balances(db: &PgPool, workspace: Uuid) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT available_micro_usd,reserved_micro_usd,refund_reserved_micro_usd FROM relay_hosted.wallets WHERE workspace_id=$1",
    ).bind(workspace).fetch_one(db).await.unwrap()
}
