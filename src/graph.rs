use std::collections::{BTreeMap, HashSet};

use crate::issue::Issue;

pub struct Graph<'a> {
    pub by_id: BTreeMap<&'a str, &'a Issue>,
}

impl<'a> Graph<'a> {
    pub fn new(issues: &'a [Issue]) -> Self {
        Graph {
            by_id: issues.iter().map(|i| (i.id.as_str(), i)).collect(),
        }
    }

    fn is_open(&self, id: &str) -> bool {
        self.by_id.get(id).is_some_and(|i| !i.closed)
    }

    /// Blockers of `issue` that are open right now.
    pub fn open_blockers(&self, issue: &Issue) -> Vec<&'a Issue> {
        issue
            .blocked_by
            .iter()
            .filter_map(|id| self.by_id.get(id.as_str()).copied())
            .filter(|b| !b.closed)
            .collect()
    }

    /// Blocker IDs that resolve to no file at all: GC'd or mistyped.
    /// Treated as satisfied; callers surface the warning.
    pub fn unknown_blockers<'b>(&self, issue: &'b Issue) -> Vec<&'b str> {
        issue
            .blocked_by
            .iter()
            .filter(|id| !self.by_id.contains_key(id.as_str()))
            .map(|id| id.as_str())
            .collect()
    }

    /// Open issues with no open blockers, priority then id order.
    pub fn ready(&self) -> Vec<&'a Issue> {
        let mut out: Vec<&Issue> = self
            .by_id
            .values()
            .filter(|i| !i.closed && self.open_blockers(i).is_empty())
            .copied()
            .collect();
        out.sort_by_key(|i| (i.priority, i.id.clone()));
        out
    }

    /// Issues that depend on `id` (reverse edges), closed dependents included
    /// so an early-closed milestone stays visible from its open parts.
    pub fn blocks(&self, id: &str) -> Vec<&'a Issue> {
        self.by_id
            .values()
            .filter(|i| i.blocked_by.iter().any(|b| b == id))
            .copied()
            .collect()
    }

    /// One cycle among open issues, as an id path, if any exists.
    pub fn find_cycle(&self) -> Option<Vec<String>> {
        let mut done: HashSet<&str> = HashSet::new();
        for &start in self.by_id.keys() {
            if done.contains(start) || !self.is_open(start) {
                continue;
            }
            let mut stack = vec![(start, 0usize)];
            let mut path = vec![start];
            while let Some((id, next)) = stack.last_mut() {
                let issue = self.by_id[*id];
                match issue.blocked_by.get(*next) {
                    Some(dep) => {
                        *next += 1;
                        let dep = dep.as_str();
                        if !self.is_open(dep) || done.contains(dep) {
                            continue;
                        }
                        if let Some(pos) = path.iter().position(|&p| p == dep) {
                            let mut cycle: Vec<String> =
                                path[pos..].iter().map(|s| s.to_string()).collect();
                            cycle.push(dep.to_string());
                            return Some(cycle);
                        }
                        stack.push((self.by_id.get_key_value(dep).unwrap().0, 0));
                        path.push(dep);
                    }
                    None => {
                        done.insert(*id);
                        stack.pop();
                        path.pop();
                    }
                }
            }
        }
        None
    }

    /// Indented blocker DAG rooted at `id`, cycle-safe.
    pub fn tree(&self, id: &str, out: &mut String) {
        let mut on_path = HashSet::new();
        self.tree_walk(id, 0, &mut on_path, out);
    }

    fn tree_walk(&self, id: &str, depth: usize, on_path: &mut HashSet<String>, out: &mut String) {
        let indent = "  ".repeat(depth);
        let Some(issue) = self.by_id.get(id) else {
            out.push_str(&format!("{indent}{id} [gone]\n"));
            return;
        };
        let state = if issue.closed { "closed" } else { "open" };
        out.push_str(&format!("{indent}{id} [{state}] {}\n", issue.title));
        if !on_path.insert(id.to_string()) {
            out.push_str(&format!("{indent}  … cycle\n"));
            return;
        }
        for dep in &issue.blocked_by {
            self.tree_walk(dep, depth + 1, on_path, out);
        }
        on_path.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::issue::parse;
    use std::path::Path;

    fn issue(id: &str, blocked_by: &[&str], closed: bool) -> Issue {
        let fm = if blocked_by.is_empty() {
            String::new()
        } else {
            format!("---\nblocked-by: [{}]\n---\n", blocked_by.join(", "))
        };
        parse(id, Path::new("x"), &format!("{fm}# {id}\n"), closed)
    }

    #[test]
    fn ready_frontier() {
        let issues = vec![
            issue("aaaaaa", &[], false),
            issue("bbbbbb", &["aaaaaa"], false),
            issue("cccccc", &["dddddd"], false), // dddd closed => ready
            issue("dddddd", &[], true),
            issue("eeeeee", &["zzzzzz"], false), // zzzz unknown => satisfied
        ];
        let g = Graph::new(&issues);
        let ready: Vec<&str> = g.ready().iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ready, ["aaaaaa", "cccccc", "eeeeee"]);
        assert_eq!(g.unknown_blockers(g.by_id["eeeeee"]), ["zzzzzz"]);
    }

    #[test]
    fn milestone_blocks_rollup() {
        let issues = vec![
            issue("mmmmmm", &["aaaaaa", "bbbbbb"], false),
            issue("aaaaaa", &[], false),
            issue("bbbbbb", &[], true),
        ];
        let g = Graph::new(&issues);
        assert_eq!(
            g.blocks("aaaaaa")
                .iter()
                .map(|i| i.id.as_str())
                .collect::<Vec<_>>(),
            ["mmmmmm"]
        );
        let mut t = String::new();
        g.tree("mmmmmm", &mut t);
        assert!(t.contains("aaaaaa [open]") && t.contains("bbbbbb [closed]"));
    }

    #[test]
    fn cycle_detection() {
        let issues = vec![
            issue("aaaaaa", &["bbbbbb"], false),
            issue("bbbbbb", &["aaaaaa"], false),
            issue("cccccc", &[], false),
        ];
        let g = Graph::new(&issues);
        let cycle = g.find_cycle().unwrap();
        assert!(cycle.len() == 3 && cycle.first() == cycle.last());
        let acyclic = vec![
            issue("aaaaaa", &["bbbbbb"], false),
            issue("bbbbbb", &[], false),
        ];
        assert!(Graph::new(&acyclic).find_cycle().is_none());
    }
}
