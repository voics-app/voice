use crate::{Clock, Config, HealthStore, MediaProbe, StoreError};
use probe_domain::{Failure, Health, Observation, Registration};

#[derive(Debug)]
pub struct Services<'a, S, M, C> {
    pub store: &'a S,
    pub media: &'a M,
    pub clock: &'a C,
}

#[derive(Debug)]
pub struct Observer<S: HealthStore> {
    pub registration: Registration,
    claim: S::Claim,
    health: Health,
}

impl<S: HealthStore> Observer<S> {
    pub async fn acquire<M: MediaProbe, C: Clock>(
        services: &Services<'_, S, M, C>,
        registration: Registration,
        config: &Config,
    ) -> Result<Self, StoreError> {
        let claim = services
            .store
            .acquire(&registration, config.lease_ttl_s())
            .await?;
        let mut health = Health::new(&registration, &config.prober_id);
        health.checked_at_ms = services.clock.now_ms();
        if let Err(error) = services.store.publish(&registration, &claim, &health).await {
            let _ = services.store.release(&claim).await;
            return Err(error);
        }
        Ok(Self {
            registration,
            claim,
            health,
        })
    }

    pub async fn observe<M: MediaProbe, C: Clock>(
        &mut self,
        services: &Services<'_, S, M, C>,
        config: &Config,
    ) -> Result<&Health, StoreError> {
        services.store.renew(&self.claim).await?;
        let result = tokio::time::timeout(
            config.timeout,
            services.media.probe(&self.registration, config),
        )
        .await
        .unwrap_or_else(|_| Observation::failed(Failure::Timeout));
        self.health.observe(
            result,
            config.failures,
            config.passes,
            services.clock.now_ms(),
        );
        services
            .store
            .publish(&self.registration, &self.claim, &self.health)
            .await?;
        Ok(&self.health)
    }

    pub async fn release(self, store: &S) -> Result<(), StoreError> {
        store.release(&self.claim).await
    }
}
