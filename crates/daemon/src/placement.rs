//! Which server each replica of a service runs on.
//!
//! [`plan`] is pure so its rules can be tested without servers: replicas that may stay where
//! they are stay, so a scale or a placement change only touches the replicas it has to, and the
//! rest go where the strategy says while each server's memory allows.

use common::{Id, PlacementStrategy};

/// A server replicas may run on, as placement sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub server_id: Id,
    /// Most replicas of the service the server can hold: the ones on it now plus what its
    /// memory fits.
    pub room: u16,
}

/// The server of each replica index `0..n`, keeping each of `current` (index, server) on its
/// server when that server still gets that many replicas. Servers are filled up to their room;
/// replicas that fit nowhere are spread over every slot anyway, so a deploy never fails on a
/// memory estimate. Returns an empty list without slots.
pub fn plan(n: u16, slots: &[Slot], strategy: PlacementStrategy, current: &[(u16, Id)]) -> Vec<Id> {
    if slots.is_empty() {
        return Vec::new();
    }
    let counts = counts(n, slots, strategy);
    let mut left = counts.clone();
    let mut out: Vec<Option<Id>> = vec![None; usize::from(n)];
    let mut kept: Vec<&(u16, Id)> = current.iter().filter(|(i, _)| *i < n).collect();
    kept.sort_by_key(|(i, _)| *i);
    for (index, server) in kept {
        if let Some(pos) = slots.iter().position(|s| &s.server_id == server)
            && left[pos] > 0
            && out[usize::from(*index)].is_none()
        {
            left[pos] -= 1;
            out[usize::from(*index)] = Some(server.clone());
        }
    }
    for slot in out.iter_mut().filter(|s| s.is_none()) {
        let pos = left
            .iter()
            .position(|&l| l > 0)
            .expect("counts add up to n");
        left[pos] -= 1;
        *slot = Some(slots[pos].server_id.clone());
    }
    out.into_iter()
        .map(|s| s.expect("every index placed"))
        .collect()
}

/// How many of `n` replicas each slot gets.
fn counts(n: u16, slots: &[Slot], strategy: PlacementStrategy) -> Vec<u16> {
    let mut counts = vec![0u16; slots.len()];
    for _ in 0..n {
        let fits = |i: usize| counts[i] < slots[i].room;
        let pick = match strategy {
            PlacementStrategy::Pack => (0..slots.len()).find(|&i| fits(i)),
            PlacementStrategy::Spread => (0..slots.len())
                .filter(|&i| fits(i))
                .min_by_key(|&i| (counts[i], u16::MAX - slots[i].room, i)),
        };
        let pick = pick.unwrap_or_else(|| {
            (0..slots.len())
                .min_by_key(|&i| (counts[i], i))
                .expect("slots is not empty")
        });
        counts[pick] += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slots(rooms: &[(&str, u16)]) -> Vec<Slot> {
        rooms
            .iter()
            .map(|(id, room)| Slot {
                server_id: (*id).into(),
                room: *room,
            })
            .collect()
    }

    fn ids(v: &[&str]) -> Vec<Id> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn spread_alternates_servers_and_prefers_the_roomiest() {
        let s = slots(&[("a", 10), ("b", 20), ("c", 10)]);
        assert_eq!(
            plan(4, &s, PlacementStrategy::Spread, &[]),
            ids(&["a", "b", "b", "c"])
        );
    }

    #[test]
    fn pack_fills_servers_in_order() {
        let s = slots(&[("a", 2), ("b", 5)]);
        assert_eq!(
            plan(4, &s, PlacementStrategy::Pack, &[]),
            ids(&["a", "a", "b", "b"])
        );
    }

    #[test]
    fn replicas_stay_where_they_are_when_they_may() {
        let s = slots(&[("a", 10), ("b", 10)]);
        let current = vec![(0, "b".into()), (1, "b".into()), (2, "a".into())];
        assert_eq!(
            plan(3, &s, PlacementStrategy::Spread, &current),
            ids(&["b", "a", "a"]),
            "one of b's two moves to even out, the rest stay"
        );
        assert_eq!(
            plan(4, &s, PlacementStrategy::Spread, &current),
            ids(&["b", "b", "a", "a"])
        );
    }

    #[test]
    fn a_removed_server_gives_its_replicas_to_the_others() {
        let s = slots(&[("a", 10), ("c", 10)]);
        let current = vec![(0, "a".into()), (1, "b".into()), (2, "c".into())];
        let placed = plan(3, &s, PlacementStrategy::Spread, &current);
        assert_eq!(placed[0], "a");
        assert_eq!(placed[2], "c");
        assert_ne!(placed[1], "b");
    }

    #[test]
    fn memory_limits_each_server_and_overflow_still_lands() {
        let s = slots(&[("a", 1), ("b", 1)]);
        assert_eq!(
            plan(3, &s, PlacementStrategy::Spread, &[]),
            ids(&["a", "a", "b"])
        );
        let s = slots(&[("small", 1), ("big", 9)]);
        assert_eq!(
            plan(3, &s, PlacementStrategy::Spread, &[]),
            ids(&["small", "big", "big"])
        );
    }

    #[test]
    fn autoscale_adds_on_the_least_loaded_server_with_room() {
        let s = slots(&[("a", 3), ("b", 3), ("c", 1)]);
        let current = vec![(0, "a".into()), (1, "b".into()), (2, "a".into())];
        assert_eq!(
            plan(4, &s, PlacementStrategy::Spread, &current)
                .pop()
                .as_deref(),
            Some("c")
        );
        let full = slots(&[("a", 2), ("b", 1)]);
        assert_eq!(
            plan(4, &full, PlacementStrategy::Spread, &current)
                .pop()
                .as_deref(),
            Some("b")
        );
    }

    #[test]
    fn no_servers_no_plan() {
        assert!(plan(2, &[], PlacementStrategy::Spread, &[]).is_empty());
    }
}
