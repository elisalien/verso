//! Layer groups, which may contain other groups.
//!
//! A layer belongs to its innermost group ([`crate::Layer::group`]); a group
//! points to the group containing it ([`crate::Group::parent`]). Grouping
//! keeps the layers of a group adjacent so the grid can show each group as
//! one block.

use crate::{Composition, Error, Group, Result};

impl Composition {
    /// `group` and the groups containing it, innermost first.
    pub fn ancestors(&self, group: Option<usize>) -> Vec<usize> {
        let mut chain = Vec::new();
        let mut current = group;
        // Bounded so a corrupted file with a parent cycle cannot hang.
        while let Some(g) = current.filter(|&g| g < self.groups.len()) {
            if chain.len() > self.groups.len() || chain.contains(&g) {
                break;
            }
            chain.push(g);
            current = self.groups[g].parent;
        }
        chain
    }

    /// Groups around `layer`, outermost first.
    pub fn layer_path(&self, layer: usize) -> Vec<usize> {
        let mut path = self.ancestors(self.layers.get(layer).and_then(|l| l.group));
        path.reverse();
        path
    }

    /// Nesting depth: 0 for a top-level group.
    pub fn group_depth(&self, group: usize) -> usize {
        self.ancestors(Some(group)).len().saturating_sub(1)
    }

    pub fn layer_in_group(&self, layer: usize, group: usize) -> bool {
        self.layer_path(layer).contains(&group)
    }

    /// Every layer inside `group`, nested groups included, bottom first.
    pub fn group_layers(&self, group: usize) -> impl Iterator<Item = usize> + '_ {
        (0..self.layers.len()).filter(move |&l| self.layer_in_group(l, group))
    }

    /// The group directly inside `scope` (or at top level) that contains
    /// `layer`, or `None` when the layer sits directly in `scope`.
    pub fn child_toward(&self, scope: Option<usize>, layer: usize) -> Option<usize> {
        let path = self.layer_path(layer);
        let start = match scope {
            None => 0,
            Some(s) => path.iter().position(|&g| g == s)? + 1,
        };
        path.get(start).copied()
    }

    /// Whether any layer or group is soloed.
    pub fn any_solo(&self) -> bool {
        self.layers.iter().any(|l| l.solo) || self.groups.iter().any(|g| g.solo)
    }

    /// Whether a solo lets `layer` through: it or a group around it is soloed.
    pub fn soloed(&self, layer: usize) -> bool {
        self.layers.get(layer).is_some_and(|l| l.solo)
            || self.layer_path(layer).iter().any(|&g| self.groups[g].solo)
    }

    /// Puts `layers` in a new group. Layers already in a group inside the
    /// common one bring their whole group along, which becomes nested. The
    /// result must be one adjacent block of layers. Returns the new group.
    pub(crate) fn group_layers_together(&mut self, layers: &[usize]) -> Result<usize> {
        if layers.is_empty() {
            return Err(Error::NotAdjacent);
        }
        if let Some(&bad) = layers.iter().find(|&&l| l >= self.layers.len()) {
            return Err(Error::NoLayer(bad));
        }
        // Deepest group around every selected layer.
        let paths: Vec<Vec<usize>> = layers.iter().map(|&l| self.layer_path(l)).collect();
        let common = (0..)
            .take_while(|&i| {
                paths[0]
                    .get(i)
                    .is_some_and(|g| paths.iter().all(|p| p.get(i) == Some(g)))
            })
            .count();
        let parent = common.checked_sub(1).map(|i| paths[0][i]);

        // What moves into the new group: whole child groups, or single layers.
        let mut groups: Vec<usize> = paths
            .iter()
            .filter_map(|p| p.get(common).copied())
            .collect();
        groups.sort_unstable();
        groups.dedup();
        let direct: Vec<usize> = layers
            .iter()
            .zip(&paths)
            .filter(|(_, p)| p.len() == common)
            .map(|(&l, _)| l)
            .collect();

        let mut members: Vec<usize> = direct.clone();
        for &g in &groups {
            members.extend(self.group_layers(g));
        }
        members.sort_unstable();
        members.dedup();
        let (low, high) = (members[0], members[members.len() - 1]);
        if high - low + 1 != members.len() {
            return Err(Error::NotAdjacent);
        }

        let index = self.groups.len();
        let mut group = Group::new(self.unique_group_name());
        group.parent = parent;
        self.groups.push(group);
        for g in groups {
            self.groups[g].parent = Some(index);
        }
        for l in direct {
            self.layers[l].group = Some(index);
        }
        Ok(index)
    }

    fn unique_group_name(&self) -> String {
        (1..)
            .map(|n| format!("Group {n}"))
            .find(|name| self.groups.iter().all(|g| &g.name != name))
            .expect("unbounded")
    }

    /// Removes `group`; its layers and groups move up to its parent.
    pub(crate) fn ungroup(&mut self, group: usize) -> Result<()> {
        let parent = self.groups.get(group).ok_or(Error::NoGroup(group))?.parent;
        // Renumber references to groups after `group`.
        let shift = |g: Option<usize>| match g {
            Some(g) if g == group => parent.map(|p| if p > group { p - 1 } else { p }),
            Some(g) if g > group => Some(g - 1),
            other => other,
        };
        for layer in &mut self.layers {
            layer.group = shift(layer.group);
        }
        for g in &mut self.groups {
            g.parent = shift(g.parent);
        }
        self.groups.remove(group);
        self.prune(|p| p.without_group(group), |a| a.without_group(group));
        Ok(())
    }

    /// Launches `column` on every layer inside `group`.
    pub(crate) fn launch_group_column(&mut self, group: usize, column: usize) -> Result<()> {
        if group >= self.groups.len() {
            return Err(Error::NoGroup(group));
        }
        if column >= self.columns.len() {
            return Err(Error::NoColumn(column));
        }
        let members: Vec<usize> = self.group_layers(group).collect();
        for l in members {
            self.layers[l].launch(column);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{Command, Composition, Error};

    /// Layers 0..n, as in the grid from the bottom.
    fn comp(layers: usize) -> Composition {
        Composition::demo_with_layers(layers)
    }

    #[test]
    fn grouping_adjacent_layers() {
        let mut c = comp(5);
        c.apply(Command::AddGroup { layers: vec![1, 2] }).unwrap();
        assert_eq!(c.layers[1].group, Some(0));
        assert_eq!(c.layers[2].group, Some(0));
        assert_eq!(c.groups[0].parent, None);
        assert_eq!(c.groups[0].name, "Group 1");
        assert!(matches!(
            c.apply(Command::AddGroup { layers: vec![0, 3] }),
            Err(Error::NotAdjacent)
        ));
    }

    #[test]
    fn grouping_a_group_with_a_neighbour_nests_it() {
        let mut c = comp(5);
        c.apply(Command::AddGroup { layers: vec![1, 2] }).unwrap();
        // Selecting one layer of the group brings the whole group.
        c.apply(Command::AddGroup { layers: vec![2, 3] }).unwrap();
        assert_eq!(c.groups[0].parent, Some(1));
        assert_eq!(c.layers[3].group, Some(1));
        assert_eq!(c.layers[1].group, Some(0));
        assert_eq!(c.layer_path(1), [1, 0]);
        assert_eq!(c.group_layers(1).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(c.group_depth(0), 1);
        assert_eq!(c.child_toward(None, 1), Some(1));
        assert_eq!(c.child_toward(Some(1), 1), Some(0));
        assert_eq!(c.child_toward(Some(1), 3), None);
    }

    #[test]
    fn grouping_inside_a_group_keeps_the_parent() {
        let mut c = comp(5);
        c.apply(Command::AddGroup {
            layers: vec![0, 1, 2, 3],
        })
        .unwrap();
        c.apply(Command::AddGroup { layers: vec![2, 3] }).unwrap();
        assert_eq!(c.groups[1].parent, Some(0));
        assert_eq!(c.layers[2].group, Some(1));
        assert_eq!(c.layers[0].group, Some(0));
    }

    #[test]
    fn ungroup_moves_contents_up_and_renumbers() {
        let mut c = comp(6);
        c.apply(Command::AddGroup { layers: vec![0] }).unwrap(); // group 0
        c.apply(Command::AddGroup {
            layers: vec![2, 3, 4],
        })
        .unwrap(); // group 1
        c.apply(Command::AddGroup { layers: vec![3, 4] }).unwrap(); // group 2 in 1
        c.apply(Command::RemoveGroup(1)).unwrap();
        // Former group 2 is now group 1, at top level.
        assert_eq!(c.groups.len(), 2);
        assert_eq!(c.groups[1].parent, None);
        assert_eq!(c.layers[3].group, Some(1));
        assert_eq!(c.layers[2].group, None);
        assert_eq!(c.layers[0].group, Some(0));
    }

    #[test]
    fn group_solo_and_column_launch() {
        // The demo's three layers all have clips.
        let mut c = comp(3);
        c.apply(Command::AddGroup { layers: vec![1, 2] }).unwrap();
        c.apply(Command::SetGroupSolo {
            group: 0,
            solo: true,
        })
        .unwrap();
        assert!(c.soloed(2) && !c.soloed(0));
        c.apply(Command::LaunchGroupColumn {
            group: 0,
            column: 1,
        })
        .unwrap();
        assert_eq!(c.layers[1].active, Some(1));
        assert_eq!(c.layers[2].active, Some(1));
        assert_eq!(c.layers[0].active, None);
        assert_eq!(c.layer_gain(0), 0.0, "soloed group mutes others");
        assert!(c.layer_gain(1) > 0.0);
    }

    #[test]
    fn parent_cycles_do_not_hang() {
        let mut c = comp(2);
        c.apply(Command::AddGroup { layers: vec![0] }).unwrap();
        c.apply(Command::AddGroup { layers: vec![0] }).unwrap();
        c.groups[1].parent = Some(0); // corrupted: 0 -> 1 -> 0
        assert!(c.layer_path(0).len() <= 2);
    }
}
