use super::*;
use tokio::sync::OwnedSemaphorePermit;

impl HttpAuthVerifier {
    pub(super) async fn evidence(&self, token: &str, force: bool) -> Verification {
        let (mut pending, waiting) = match self.prepare(token, force)? {
            Prepared::Ready(evidence) => return Ok(evidence),
            Prepared::Pending(receiver, permit) => (receiver, permit),
        };
        let result = tokio::time::timeout(NETWORK_DEADLINE * 2, async {
            pending
                .wait_for(|value| value.is_some())
                .await
                .map_err(|_| AuthError::Unavailable)?
                .clone()
                .ok_or(AuthError::Unavailable)?
        })
        .await
        .map_err(|_| AuthError::Unavailable)?;
        drop(waiting);
        result
    }

    fn prepare(&self, token: &str, force: bool) -> Result<Prepared, AuthError> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(AuthError::Closed);
        }
        let digest = token_digest(token)?;
        let mut cache = self
            .inner
            .cache
            .lock()
            .map_err(|_| AuthError::Unavailable)?;
        cache.retain(|_, entry| {
            entry.pending.is_some()
                || entry
                    .evidence
                    .as_ref()
                    .is_some_and(|e| e.deadline > Instant::now())
        });
        if let Some(evidence) = cached(&cache, &digest, force) {
            return Ok(Prepared::Ready(evidence));
        }
        let waiting = self
            .inner
            .waiters
            .clone()
            .try_acquire_owned()
            .map_err(|_| AuthError::Overloaded)?;
        if cache.len() >= CACHE_LIMIT && !cache.contains_key(&digest) {
            return Err(AuthError::Overloaded);
        }
        if let Some(receiver) = cache.get(&digest).and_then(|entry| entry.pending.as_ref()) {
            return Ok(Prepared::Pending(receiver.clone(), waiting));
        }
        // The task owns this permit, so cancelling its caller cannot expand the
        // background queue. Waiters and outstanding work have separate limits.
        let job = self
            .inner
            .jobs
            .clone()
            .try_acquire_owned()
            .map_err(|_| AuthError::Overloaded)?;
        let entry = cache.entry(digest).or_insert_with(|| CacheEntry {
            state: Arc::new(TokenState {
                revoked: AtomicBool::new(false),
                verifier_closed: self.inner.closed.clone(),
            }),
            evidence: None,
            pending: None,
        });
        let (sender, receiver) = watch::channel(None);
        entry.pending = Some(receiver.clone());
        self.start_lookup(digest, token.to_owned(), entry.state.clone(), sender, job);
        Ok(Prepared::Pending(receiver, waiting))
    }

    fn start_lookup(
        &self,
        digest: TokenDigest,
        token: String,
        state: Arc<TokenState>,
        sender: watch::Sender<Option<Verification>>,
        job: OwnedSemaphorePermit,
    ) {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let result = inner.lookup(&token, digest, state.clone()).await;
            let result = inner.finish(digest, &state, result);
            let _ = sender.send(Some(result));
            drop(job);
        });
    }
}

fn cached(
    cache: &HashMap<TokenDigest, CacheEntry>,
    digest: &TokenDigest,
    force: bool,
) -> Option<Arc<Evidence>> {
    if force {
        return None;
    }
    let entry = cache.get(digest)?;
    let evidence = entry.evidence.as_ref()?;
    (!entry.state.revoked.load(Ordering::Acquire) && evidence.deadline > Instant::now())
        .then(|| evidence.clone())
}

impl VerifierInner {
    fn finish(
        &self,
        digest: TokenDigest,
        state: &Arc<TokenState>,
        mut result: Verification,
    ) -> Verification {
        let Ok(mut cache) = self.cache.lock() else {
            return Err(AuthError::Unavailable);
        };
        let Some(entry) = cache.get_mut(&digest) else {
            return Err(AuthError::Unavailable);
        };
        if !Arc::ptr_eq(&entry.state, state) {
            return Err(AuthError::Unavailable);
        }
        if let (Some(old), Ok(new)) = (&entry.evidence, &result) {
            if old.binding != new.binding {
                result = Err(AuthError::BindingMismatch);
            }
        }
        entry.pending = None;
        match &result {
            Ok(evidence) => entry.evidence = Some(evidence.clone()),
            Err(AuthError::InvalidCredential | AuthError::BindingMismatch | AuthError::Expired) => {
                state.revoked.store(true, Ordering::Release);
                cache.remove(&digest);
            }
            Err(_) if entry.evidence.is_none() => {
                cache.remove(&digest);
            }
            Err(_) => (),
        }
        result
    }
}
