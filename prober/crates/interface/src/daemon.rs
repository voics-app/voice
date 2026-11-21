//! Each discovered node owns its own timed loop; a slow target cannot delay another node.
use probe_application::{
    Clock, Config, MediaProbe, NodeDiscovery, ObservationSink, Observer, Services,
};
use probe_domain::Registration;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::watch;
use tokio::task::JoinHandle;

#[derive(Debug)]
pub struct Runtime<S, M, C> {
    pub config: Arc<Config>,
    pub store: Arc<S>,
    pub media: Arc<M>,
    pub clock: Arc<C>,
    pub sink: Arc<dyn ObservationSink>,
}

struct Worker {
    registration: watch::Sender<Option<Registration>>,
    task: JoinHandle<()>,
}

impl<S: NodeDiscovery + 'static, M: MediaProbe + 'static, C: Clock + 'static> Runtime<S, M, C> {
    pub async fn run(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        let mut workers = HashMap::<String, Worker>::new();
        let mut interval = tokio::time::interval(self.config.interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                changed = shutdown.changed() => if changed.is_err() || *shutdown.borrow() { break; },
                _ = interval.tick() => {
                    match self.store.registrations().await {
                        Ok(registrations) => self.reconcile(&mut workers, registrations).await,
                        Err(error) => eprintln!("voice discovery unavailable: {error:?}"),
                    }
                }
            }
        }
        for worker in workers.values() {
            let _ = worker.registration.send(None);
        }
        for (_, worker) in workers {
            let _ = worker.task.await;
        }
    }

    async fn reconcile(
        self: &Arc<Self>,
        workers: &mut HashMap<String, Worker>,
        registrations: Vec<Registration>,
    ) {
        let present: std::collections::HashSet<_> = registrations
            .iter()
            .map(|r| r.node.node_id.clone())
            .collect();
        let removed: Vec<_> = workers
            .keys()
            .filter(|id| !present.contains(*id))
            .cloned()
            .collect();
        for id in removed {
            if let Some(worker) = workers.remove(&id) {
                let _ = worker.registration.send(None);
                let _ = worker.task.await;
            }
        }
        for registration in registrations {
            let id = registration.node.node_id.clone();
            if let Some(worker) = workers.get(&id) {
                worker.registration.send_if_modified(|current| {
                    if current.as_ref() == Some(&registration) {
                        false
                    } else {
                        *current = Some(registration.clone());
                        true
                    }
                });
            } else {
                let (sender, receiver) = watch::channel(Some(registration));
                let runtime = Arc::clone(self);
                let task = tokio::spawn(async move { runtime.run_node(receiver).await });
                workers.insert(
                    id,
                    Worker {
                        registration: sender,
                        task,
                    },
                );
            }
        }
    }

    async fn run_node(&self, mut registration: watch::Receiver<Option<Registration>>) {
        let mut observer = None::<Observer<S>>;
        let mut interval = tokio::time::interval(self.config.interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let services = Services {
            store: self.store.as_ref(),
            media: self.media.as_ref(),
            clock: self.clock.as_ref(),
        };
        let mut node_id = None;
        loop {
            tokio::select! {
                changed = registration.changed() => {
                    if changed.is_err() || registration.borrow().is_none() { break; }
                    interval.reset_immediately();
                }
                _ = interval.tick() => {
                    let Some(current) = registration.borrow().clone() else { break; };
                    node_id = Some(current.node.node_id.clone());
                    if observer.as_ref().is_some_and(|o| o.registration != current) {
                        self.release(observer.take()).await;
                    }
                    if observer.is_none() {
                        match Observer::acquire(&services, current.clone(), &self.config).await {
                            Ok(owned) => observer = Some(owned),
                            Err(error) => {
                                self.sink.report(&current, None);
                                eprintln!("probe ownership unavailable for {:?}: {error:?}", current.node.node_id);
                                continue;
                            }
                        }
                    }
                    let observation = tokio::select! {
                        biased;
                        _ = registration.changed() => None,
                        result = observer.as_mut().expect("observer acquired").observe(&services, &self.config) => Some(result.cloned()),
                    };
                    match observation {
                        Some(Ok(health)) => self.sink.report(&current, Some(&health)),
                        Some(Err(error)) => {
                            self.sink.report(&current, None);
                            eprintln!("probe observation fenced for {:?}: {error:?}", current.node.node_id);
                            self.release(observer.take()).await;
                        }
                        None => {
                            // Dropping the probe closes its sockets before revoking ownership.
                            self.release(observer.take()).await;
                            self.sink.report(&current, None);
                            if registration.borrow().is_none() { break; }
                            interval.reset_immediately();
                        }
                    }
                }
            }
        }
        self.release(observer).await;
        if let Some(id) = node_id {
            self.sink.remove(&id);
        }
    }

    async fn release(&self, observer: Option<Observer<S>>) {
        if let Some(observer) = observer
            && let Err(error) = observer.release(self.store.as_ref()).await
        {
            eprintln!("probe lease release failed; bounded lease will expire: {error:?}");
        }
    }
}
