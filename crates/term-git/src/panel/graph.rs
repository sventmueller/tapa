//! The commit graph beside the history, as VS Code draws it: every commit a dot in its lane, lines from each commit
//! down to its parents, a lane's colour kept for as long as the lane runs.

use crate::{Commit, CommitId};

/// A line in one half of a row: from a lane at one edge to a lane at the other, in the colour of `color`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    /// The lane it starts in.
    pub from: usize,
    /// The lane it ends in.
    pub to: usize,
    /// Its lane colour, an index the view turns into a colour.
    pub color: usize,
}

/// How one commit's row of the graph looks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphRow {
    /// The lane of the commit's dot.
    pub lane: usize,
    /// The lane colour of the dot.
    pub color: usize,
    /// How many lanes the row needs.
    pub width: usize,
    /// Lines from the top edge to the middle; those that end in the dot's lane end at the dot.
    pub top: Vec<Segment>,
    /// Lines from the middle to the bottom edge; those that start in the dot's lane start at the dot.
    pub bottom: Vec<Segment>,
    /// Whether the commit has more than one parent.
    pub merge: bool,
    /// Whether HEAD points at the commit.
    pub head: bool,
}

impl GraphRow {
    /// The lanes that run on below the commit, as straight lines through rows shown beneath it.
    pub fn through(&self) -> GraphRow {
        let mut lanes: Vec<Segment> = self
            .bottom
            .iter()
            .map(|s| Segment {
                from: s.to,
                to: s.to,
                color: s.color,
            })
            .collect();
        lanes.sort_by_key(|s| s.to);
        lanes.dedup_by_key(|s| s.to);
        GraphRow {
            lane: usize::MAX,
            color: 0,
            width: self.width,
            top: lanes.clone(),
            bottom: lanes,
            merge: false,
            head: false,
        }
    }
}

/// The rows of `commits`, which come children before parents.
pub(crate) fn layout(commits: &[Commit]) -> Vec<GraphRow> {
    let mut lanes: Vec<Option<(CommitId, usize)>> = Vec::new();
    let mut next_color = 0;
    let mut rows = Vec::with_capacity(commits.len());
    for commit in commits {
        let before = lanes.clone();
        let found = lanes
            .iter()
            .position(|l| l.as_ref().is_some_and(|(h, _)| *h == commit.hash));
        let (lane, color) = match found {
            Some(lane) => (lane, before[lane].as_ref().map_or(0, |(_, c)| *c)),
            None => {
                let color = next_color;
                next_color += 1;
                (free_lane(&mut lanes, None), color)
            }
        };
        let mut top = Vec::new();
        for (index, entry) in before.iter().enumerate() {
            let Some((hash, lane_color)) = entry else { continue };
            let to = if *hash == commit.hash { lane } else { index };
            top.push(Segment {
                from: index,
                to,
                color: *lane_color,
            });
        }
        for entry in &mut lanes {
            if entry.as_ref().is_some_and(|(h, _)| *h == commit.hash) {
                *entry = None;
            }
        }
        let mut bottom = Vec::new();
        for (index, parent) in commit.parents.iter().enumerate() {
            // The first parent keeps the commit's lane even when another lane waits for it already: both lanes run
            // into the parent's dot, and the line of the branch being followed stays straight.
            let existing = lanes
                .iter()
                .position(|l| l.as_ref().is_some_and(|(h, _)| h == parent))
                .filter(|_| index > 0);
            let (to, segment_color) = match existing {
                Some(to) => (to, lanes[to].as_ref().map_or(0, |(_, c)| *c)),
                None if index == 0 => {
                    lanes[lane] = Some((parent.clone(), color));
                    (lane, color)
                }
                None => {
                    let new_color = next_color;
                    next_color += 1;
                    let to = free_lane(&mut lanes, Some(lane));
                    lanes[to] = Some((parent.clone(), new_color));
                    (to, new_color)
                }
            };
            bottom.push(Segment {
                from: lane,
                to,
                color: segment_color,
            });
        }
        for (index, entry) in before.iter().enumerate() {
            if let Some((hash, lane_color)) = entry
                && *hash != commit.hash
            {
                bottom.push(Segment {
                    from: index,
                    to: index,
                    color: *lane_color,
                });
            }
        }
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        let width = before.len().max(lanes.len()).max(lane + 1);
        rows.push(GraphRow {
            lane,
            color,
            width,
            top,
            bottom,
            merge: commit.parents.len() > 1,
            head: commit.refs.iter().any(|r| r == "HEAD" || r.starts_with("HEAD -> ")),
        });
    }
    rows
}

/// The first empty lane other than `except`, or a new one at the end.
fn free_lane(lanes: &mut Vec<Option<(CommitId, usize)>>, except: Option<usize>) -> usize {
    if let Some(index) = (0..lanes.len()).find(|i| lanes[*i].is_none() && Some(*i) != except) {
        return index;
    }
    lanes.push(None);
    lanes.len() - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(hash: &str, parents: &[&str]) -> Commit {
        Commit {
            hash: CommitId::from_git(hash),
            parents: parents.iter().map(|p| CommitId::from_git(*p)).collect(),
            subject: String::new(),
            body: String::new(),
            author_name: String::new(),
            author_email: String::new(),
            author_time: 0,
            refs: if hash == "m" {
                vec!["HEAD -> main".into()]
            } else {
                vec![]
            },
        }
    }

    #[test]
    fn a_straight_history_stays_in_one_lane() {
        let rows = layout(&[commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])]);
        assert!(rows.iter().all(|r| r.lane == 0 && r.width == 1 && r.color == 0));
        assert!(rows[0].top.is_empty());
        assert_eq!(
            rows[1].top,
            [Segment {
                from: 0,
                to: 0,
                color: 0
            }]
        );
        assert!(rows[2].bottom.is_empty());
    }

    #[test]
    fn a_merge_opens_a_lane_for_its_second_parent_that_joins_where_both_meet() {
        // m merges x into b; x and b both come from a.
        let rows = layout(&[
            commit("m", &["b", "x"]),
            commit("x", &["a"]),
            commit("b", &["a"]),
            commit("a", &[]),
        ]);
        assert!(rows[0].merge && rows[0].head);
        assert_eq!(
            rows[0].bottom,
            [
                Segment {
                    from: 0,
                    to: 0,
                    color: 0
                },
                Segment {
                    from: 0,
                    to: 1,
                    color: 1
                }
            ]
        );
        assert_eq!((rows[1].lane, rows[1].color), (1, 1));
        assert_eq!(rows[1].width, 2);
        assert_eq!((rows[2].lane, rows[2].color), (0, 0));
        assert!(rows[2].bottom.contains(&Segment {
            from: 1,
            to: 1,
            color: 1
        }));
        assert_eq!(rows[3].lane, 0);
        assert!(rows[3].top.contains(&Segment {
            from: 1,
            to: 0,
            color: 1
        }));
        assert_eq!(rows[3].width, 2);
    }

    #[test]
    fn a_second_tip_starts_its_own_lane_and_rows_beneath_a_commit_carry_its_lanes_on() {
        // u is ahead on the upstream, m on HEAD; both come from a.
        let rows = layout(&[commit("u", &["a"]), commit("m", &["a"]), commit("a", &[])]);
        assert_eq!((rows[0].lane, rows[1].lane), (0, 1));
        assert_ne!(rows[0].color, rows[1].color);
        let through = rows[1].through();
        assert_eq!(through.top, through.bottom);
        assert_eq!(through.top.iter().map(|s| s.to).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(rows[2].top.len(), 2);
    }
}
