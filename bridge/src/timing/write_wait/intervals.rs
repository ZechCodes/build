use super::*;

#[derive(Clone, Copy)]
pub(super) struct Span {
    pub start: Instant,
    pub end: Instant,
}

#[derive(Clone, Default)]
pub(super) struct ClosedIntervals {
    known: Vec<Span>,
    sql: Vec<Span>,
    external: Vec<Span>,
    longest: Duration,
}

impl ClosedIntervals {
    pub(super) fn insert(&mut self, interval: &ActiveInterval, now: Instant) {
        let span = Span {
            start: interval.started,
            end: now.max(interval.started),
        };
        match interval.category {
            IntervalCategory::KnownWait(_) => self.known.push(span),
            IntervalCategory::SqlService => self.sql.push(span),
            IntervalCategory::OwnExternal | IntervalCategory::FiniteService => {
                self.external.push(span)
            }
        }
        if matches!(
            interval.category,
            IntervalCategory::KnownWait(_) | IntervalCategory::SqlService
        ) {
            self.longest = self.longest.max(span.end.duration_since(span.start));
        }
    }

    pub(super) fn measure(
        &self,
        active: &[(ActiveInterval, Option<Instant>)],
        now: Instant,
    ) -> Totals {
        let mut known = self.known.clone();
        let mut sql = self.sql.clone();
        let mut external = self.external.clone();
        let mut longest = self.longest;
        for (interval, ended) in active {
            let span = Span {
                start: interval.started,
                end: ended.unwrap_or(now).min(now).max(interval.started),
            };
            match interval.category {
                IntervalCategory::KnownWait(_) => known.push(span),
                IntervalCategory::SqlService => sql.push(span),
                IntervalCategory::OwnExternal | IntervalCategory::FiniteService => {
                    external.push(span)
                }
            }
            if matches!(
                interval.category,
                IntervalCategory::KnownWait(_) | IntervalCategory::SqlService
            ) {
                longest = longest.max(span.end.duration_since(span.start));
            }
        }
        let known_total = union(&known);
        let sql_total = union(&sql);
        let external_total = union(&external);
        known.extend(sql);
        let blocked = union(&known);
        known.extend(external);
        Totals {
            known: known_total,
            sql: sql_total,
            blocked,
            external: external_total,
            external_active: union(&known).saturating_sub(blocked),
            longest,
        }
    }
}

pub(super) struct Totals {
    pub known: Duration,
    pub sql: Duration,
    pub blocked: Duration,
    pub external: Duration,
    pub external_active: Duration,
    pub longest: Duration,
}

fn union(spans: &[Span]) -> Duration {
    let mut spans = spans.to_vec();
    spans.sort_unstable_by_key(|span| span.start);
    let Some(mut current) = spans.first().copied() else {
        return Duration::ZERO;
    };
    let mut total = Duration::ZERO;
    for span in spans.into_iter().skip(1) {
        if span.start <= current.end {
            current.end = current.end.max(span.end);
        } else {
            total += current.end.duration_since(current.start);
            current = span;
        }
    }
    total + current.end.duration_since(current.start)
}
