use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RendererEventKind {
    Pointer,
    Mpris,
    MprisV2,
    Audio,
}

impl RendererEventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pointer => "pointer",
            Self::Mpris => "mpris",
            Self::MprisV2 => "mpris-v2",
            Self::Audio => "audio",
        }
    }

    pub(super) fn parse(value: &str) -> Option<Self> {
        match value {
            "pointer" => Some(Self::Pointer),
            "mpris" => Some(Self::Mpris),
            "mpris-v2" => Some(Self::MprisV2),
            "audio" => Some(Self::Audio),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RendererSubscription {
    pub revision: u64,
    pub kinds: BTreeSet<RendererEventKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct RendererSubscriptionEntry {
    subscription: RendererSubscription,
    membership_generations: BTreeMap<RendererEventKind, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RendererSubscriptionSnapshot {
    entries: Arc<BTreeMap<RendererId, RendererSubscriptionEntry>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RendererProcessOwnershipSnapshot {
    pub generation: u64,
    pub(super) process_groups: Arc<BTreeSet<i32>>,
}

impl RendererProcessOwnershipSnapshot {
    pub fn owns_process_group(&self, process_group: i32) -> bool {
        self.process_groups.contains(&process_group)
    }

    #[cfg(test)]
    pub(crate) fn from_process_groups(process_groups: impl IntoIterator<Item = i32>) -> Self {
        Self {
            generation: 0,
            process_groups: Arc::new(process_groups.into_iter().collect()),
        }
    }
}

impl RendererSubscriptionSnapshot {
    /// Returns the committed complete-set wire revision for a current subscriber.
    pub fn wire_revision_for(&self, id: &str, kind: RendererEventKind) -> Option<u64> {
        self.entries
            .get(id)
            .filter(|entry| entry.subscription.kinds.contains(&kind))
            .map(|entry| entry.subscription.revision)
    }

    /// Lists current subscribers with revisions used to reject stale wire payloads.
    pub fn subscribers_with_wire_revision(
        &self,
        kind: RendererEventKind,
    ) -> Vec<(RendererId, u64)> {
        self.entries
            .iter()
            .filter_map(|(id, entry)| {
                entry
                    .subscription
                    .kinds
                    .contains(&kind)
                    .then(|| (id.clone(), entry.subscription.revision))
            })
            .collect()
    }

    /// Returns the generation of the latest membership change for this event kind.
    pub fn membership_generation_for(&self, id: &str, kind: RendererEventKind) -> Option<u64> {
        self.entries
            .get(id)
            .filter(|entry| entry.subscription.kinds.contains(&kind))
            .and_then(|entry| entry.membership_generations.get(&kind).copied())
    }

    /// Lists subscribers without coupling them to revisions of unrelated event kinds.
    pub fn subscribers_with_membership_generation(
        &self,
        kind: RendererEventKind,
    ) -> Vec<(RendererId, u64)> {
        self.entries
            .iter()
            .filter_map(|(id, entry)| {
                if !entry.subscription.kinds.contains(&kind) {
                    return None;
                }
                entry
                    .membership_generations
                    .get(&kind)
                    .copied()
                    .map(|generation| (id.clone(), generation))
            })
            .collect()
    }
}

pub(super) struct SubscriptionApply {
    pub(super) revision: u64,
    pub(super) status: EventSubscriptionStatus,
    pub(super) kinds: Vec<String>,
    pub(super) reason: String,
    pub(super) commit: Option<RendererSubscription>,
}

pub(super) struct RendererSubscriptionRegistry {
    committed: StdMutex<BTreeMap<RendererId, RendererSubscriptionEntry>>,
    published: watch::Sender<RendererSubscriptionSnapshot>,
    next_membership_generation: AtomicU64,
}

impl RendererSubscriptionRegistry {
    pub(super) fn new() -> Self {
        let (published, _) = watch::channel(RendererSubscriptionSnapshot::default());
        Self {
            committed: StdMutex::new(BTreeMap::new()),
            published,
            next_membership_generation: AtomicU64::new(1),
        }
    }

    pub(super) fn register(&self, id: RendererId) {
        if let Ok(mut committed) = self.committed.lock() {
            committed.insert(id, RendererSubscriptionEntry::default());
            self.publish_locked(&committed);
        }
    }

    pub(super) fn remove(&self, id: &str) {
        if let Ok(mut committed) = self.committed.lock() {
            committed.remove(id);
            self.publish_locked(&committed);
        }
    }

    pub(super) fn prepare(
        &self,
        id: &str,
        revision: u64,
        raw_kinds: &[String],
    ) -> SubscriptionApply {
        let committed = match self.committed.lock() {
            Ok(committed) => committed,
            Err(_) => {
                return SubscriptionApply {
                    revision,
                    status: EventSubscriptionStatus::Invalid,
                    kinds: Vec::new(),
                    reason: "subscription registry unavailable".to_string(),
                    commit: None,
                };
            }
        };
        let Some(current) = committed.get(id) else {
            return SubscriptionApply {
                revision,
                status: EventSubscriptionStatus::Invalid,
                kinds: Vec::new(),
                reason: "renderer is not registered".to_string(),
                commit: None,
            };
        };
        let current_kinds = || {
            current
                .subscription
                .kinds
                .iter()
                .map(|kind| kind.as_str().to_string())
                .collect()
        };
        if raw_kinds.len() > MAX_EVENT_SUBSCRIPTIONS {
            return SubscriptionApply {
                revision,
                status: EventSubscriptionStatus::LimitExceeded,
                kinds: current_kinds(),
                reason: format!("at most {MAX_EVENT_SUBSCRIPTIONS} event kinds are allowed"),
                commit: None,
            };
        }
        if revision == 0 {
            return SubscriptionApply {
                revision,
                status: EventSubscriptionStatus::Invalid,
                kinds: current_kinds(),
                reason: "revision must start at 1".to_string(),
                commit: None,
            };
        }
        let total_kind_bytes = raw_kinds
            .iter()
            .try_fold(0usize, |total, kind| total.checked_add(kind.len()));
        if total_kind_bytes.is_none_or(|total| total > MAX_EVENT_KIND_TOTAL_BYTES)
            || raw_kinds
                .iter()
                .any(|kind| kind.len() > MAX_EVENT_KIND_BYTES)
        {
            return SubscriptionApply {
                revision,
                status: EventSubscriptionStatus::LimitExceeded,
                kinds: current_kinds(),
                reason: format!(
                    "event kind names are limited to {MAX_EVENT_KIND_BYTES} bytes each and \
                     {MAX_EVENT_KIND_TOTAL_BYTES} bytes total"
                ),
                commit: None,
            };
        }

        let mut kinds = BTreeSet::new();
        for raw in raw_kinds {
            let Some(kind) = RendererEventKind::parse(raw) else {
                return SubscriptionApply {
                    revision,
                    status: EventSubscriptionStatus::Invalid,
                    kinds: current_kinds(),
                    reason: format!("unknown event kind {raw:?}"),
                    commit: None,
                };
            };
            kinds.insert(kind);
        }
        let canonical: Vec<String> = kinds.iter().map(|kind| kind.as_str().to_string()).collect();

        if revision < current.subscription.revision {
            return SubscriptionApply {
                revision,
                status: EventSubscriptionStatus::StaleRevision,
                kinds: current_kinds(),
                reason: format!("current revision is {}", current.subscription.revision),
                commit: None,
            };
        }
        if revision == current.subscription.revision {
            let same = kinds == current.subscription.kinds;
            return SubscriptionApply {
                revision,
                status: if same {
                    EventSubscriptionStatus::Applied
                } else {
                    EventSubscriptionStatus::RevisionConflict
                },
                kinds: current_kinds(),
                reason: if same {
                    String::new()
                } else {
                    "revision already names a different event set".to_string()
                },
                commit: None,
            };
        }

        SubscriptionApply {
            revision,
            status: EventSubscriptionStatus::Applied,
            kinds: canonical,
            reason: String::new(),
            commit: Some(RendererSubscription { revision, kinds }),
        }
    }

    pub(super) fn commit(&self, id: RendererId, subscription: RendererSubscription) {
        if let Ok(mut committed) = self.committed.lock() {
            let Some(current) = committed.get(&id).cloned() else {
                return;
            };
            let mut membership_generations = current.membership_generations;
            for kind in [
                RendererEventKind::Pointer,
                RendererEventKind::Mpris,
                RendererEventKind::MprisV2,
                RendererEventKind::Audio,
            ] {
                if current.subscription.kinds.contains(&kind) != subscription.kinds.contains(&kind)
                {
                    membership_generations.insert(
                        kind,
                        self.next_membership_generation
                            .fetch_add(1, Ordering::Relaxed),
                    );
                }
            }
            committed.insert(
                id,
                RendererSubscriptionEntry {
                    subscription,
                    membership_generations,
                },
            );
            self.publish_locked(&committed);
        }
    }

    pub(super) fn snapshot(&self) -> RendererSubscriptionSnapshot {
        self.published.borrow().clone()
    }

    pub(super) fn subscribe(&self) -> watch::Receiver<RendererSubscriptionSnapshot> {
        self.published.subscribe()
    }

    fn publish_locked(&self, committed: &BTreeMap<RendererId, RendererSubscriptionEntry>) {
        self.published.send_replace(RendererSubscriptionSnapshot {
            entries: Arc::new(committed.clone()),
        });
    }
}
