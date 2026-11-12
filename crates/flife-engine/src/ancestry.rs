//! The live population's ancestry (FL-021, plan §14.2's Descendants): how
//! many descendants an organism has and the ancestors its line passes
//! through, for the inspector.
//!
//! A world that traces its ancestry
//! ([`World::trace_ancestry`](crate::World::trace_ancestry)) keeps an index
//! it folds from every tick it publishes: the tick's births, then its
//! deaths, then its introductions, as its report lists them. Like the watch,
//! the index is not scientific state: no digest, checkpoint or report holds
//! it, and a step reads nothing from it, so tracing changes no result of a
//! run (plan §21.2, INV-09).
//!
//! The index holds every live organism and the dead ancestors at which two
//! or more lines that still have live members meet. Each member of the
//! index knows its nearest ancestor in the index, which for a live organism
//! with a live parent is that parent. When an organism dies, it stays only
//! if two or more of its lines live on: with none it is dropped, and the
//! ancestor above it may lose a line too; with one, its place passes to
//! that line. So the index holds at most `2n − 1` organisms for `n` live
//! ones, never more than twice `population.maximum_count`. Its room,
//! [`ROOM_PER_ORGANISM`] entries for each organism the population can hold,
//! is reserved when tracing begins: the members, those that have just left,
//! and the tick's newcomers. Leavers are cleared out before newcomers are
//! added when the room would not hold them, so folding a tick never
//! allocates.
//!
//! For each member the index counts the children it bore and how many of
//! them live, and the descendants born below it and how many of them live.
//! Births count from the committed tick tracing began, its `since`: a world
//! traced from its founding counts every birth. Tracing that begins later
//! starts from the live population, each organism under its parent when
//! that parent lives too, so an organism whose parent died before then
//! starts a line of its own, and the descendants it counts live are those
//! reached through organisms alive since then.
//!
//! Folding a tick costs, for each birth and death, one step for each member
//! between it and its line's first ancestor in the index, besides a binary
//! search for each member it finds; clearing out leavers costs a pass over
//! the room, at most once for every [`ROOM_PER_ORGANISM`] − 2 times the
//! population's capacity newcomers. A [`Descendants`] answer costs one step
//! for each ancestor above the organism in the index.
//!
//! The index can also follow one organism's line of descent (plan §2.2,
//! "Follow descendants of a selected organism"): a [`Followed`] line. When
//! the organism that has the line dies, the line passes to its heir, the
//! descendant that is alive once the tick's deaths are all folded, fewest
//! generations below it and, among those, the eldest (the lowest
//! identity). Its descendants are those the index reaches through
//! organisms alive since `since`, as [`Descendants::living`] counts them.
//! With none alive, the line ends there. Finding the heir costs one pass
//! over the entries above the dead organism, with a binary search for each.

use flife_core::CoreError;
use flife_core::clock::Tick;
use flife_core::ids::{Generation, OrganismId};
use flife_core::memory::{bytes_for, capacity_bytes};

use crate::population::Population;
use crate::world::TickReport;

/// The most ancestors a [`Descendants`] names, nearest first.
pub const ANCESTRY_PATH: usize = 8;

/// The index entries reserved for each organism the population can hold:
/// two for the members (a live organism and at most one dead ancestor
/// where lines meet) and one for those that have left and the tick's
/// newcomers.
pub const ROOM_PER_ORGANISM: usize = 3;

/// What the index knows of one live organism's descendants and ancestors
/// at a committed tick (FL-021, plan §14.2's Descendants).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Descendants {
    /// The committed tick tracing began at: births are counted from it.
    pub since: Tick,
    /// The children it bore after `since`.
    pub children_born: u64,
    /// Its children alive now, whenever they were born.
    pub children_living: u64,
    /// Its descendants born after `since`: its children, theirs, and so on.
    pub born: u64,
    /// Its descendants alive now, reached through organisms alive at or
    /// after `since`.
    pub living: u64,
    /// The ancestors its line passes through in the index, nearest first,
    /// at most [`ANCESTRY_PATH`]: its live ancestors and the dead ones at
    /// which its line meets another that still has live members.
    pub path: Vec<Ancestor>,
    /// The ancestors in the index above the last of `path`.
    pub beyond: u64,
}

/// One ancestor a [`Descendants`] path passes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ancestor {
    /// Its stable identity.
    pub organism: OrganismId,
    /// Its generation.
    pub generation: Generation,
    /// The first committed tick it existed in.
    pub birth_tick: Tick,
    /// Whether it lives.
    pub alive: bool,
    /// Its descendants born after the index's `since`.
    pub born: u64,
    /// Its descendants alive now.
    pub living: u64,
}

/// How much of its room the index holds ([`crate::World::ancestry_held`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AncestryHeld {
    /// The committed tick tracing began at.
    pub since: Tick,
    /// Its members: live organisms and the dead ancestors where lines meet.
    pub members: usize,
    /// Of them, the dead ones.
    pub dead: usize,
    /// Its entries, members and those that have left and are not yet
    /// cleared out.
    pub entries: usize,
    /// The entries its room holds.
    pub room: usize,
    /// The bytes that room takes.
    pub bytes: u64,
}

/// A line of descent the index follows ([`crate::World::follow`]; plan
/// §2.2, "Follow descendants of a selected organism").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Followed {
    /// The organism the line was followed from.
    pub origin: OrganismId,
    /// The organism that has the line: the origin, or the heir of the one
    /// that had it before.
    pub organism: OrganismId,
    /// The committed tick `organism` took the line at: the tick following
    /// began, or the tick the one before it died.
    pub since: Tick,
    /// How many times the line has passed to an heir.
    pub handoffs: u64,
    /// Whether `organism` lives. When it died with no descendant alive the
    /// line ends: it stays, naming it, until another is followed.
    pub alive: bool,
}

/// An entry's standing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// A live organism.
    Living,
    /// A dead ancestor at which two or more lines with live members meet.
    Meeting,
    /// A dead ancestor that has left with one line living on: a link to it
    /// stands for a link to its own nearest ancestor.
    Passed,
    /// A dead organism that has left with no line living on.
    Dropped,
}

/// One entry.
#[derive(Debug, Clone, Copy)]
struct Entry {
    organism: OrganismId,
    /// Its parent, from its row; `None` for a founder or an introduction.
    parent: Option<OrganismId>,
    /// Its nearest ancestor in the index as last resolved; a
    /// [`Standing::Passed`] one stands for its own.
    link: Option<OrganismId>,
    generation: Generation,
    birth_tick: Tick,
    standing: Standing,
    /// Its lines: members whose nearest ancestor in the index it is.
    lines: u32,
    children_born: u64,
    children_living: u64,
    born: u64,
    living: u64,
    /// Scratch for finding an heir: whether it descends from the organism
    /// whose heir is sought. Only entries above that organism's are read,
    /// each after it is written.
    descends: bool,
}

impl Entry {
    const fn new(
        organism: OrganismId,
        parent: Option<OrganismId>,
        generation: Generation,
        birth_tick: Tick,
    ) -> Self {
        Self {
            organism,
            parent,
            link: None,
            generation,
            birth_tick,
            standing: Standing::Living,
            lines: 0,
            children_born: 0,
            children_living: 0,
            born: 0,
            living: 0,
            descends: false,
        }
    }

    const fn member(&self) -> bool {
        matches!(self.standing, Standing::Living | Standing::Meeting)
    }
}

/// The index (see the module documentation).
#[derive(Debug)]
pub(crate) struct Ancestry {
    since: Tick,
    /// Ascending by organism: newcomers' identities exceed every one issued
    /// before them, and clearing out keeps the order.
    entries: Vec<Entry>,
    /// The newest identity the index has taken.
    newest: Option<OrganismId>,
    /// The line it follows, if any.
    followed: Option<Followed>,
}

impl Ancestry {
    /// Begin at committed tick `since` from `population`, reserving room
    /// for its capacity.
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when the room cannot be reserved.
    pub(crate) fn begin(population: &Population, since: Tick) -> Result<Self, CoreError> {
        let mut ancestry = Self::with_room(
            population.capacity().saturating_mul(ROOM_PER_ORGANISM),
            since,
        )?;
        // In identity order: a parent's identity is below its child's, so
        // a live parent is already in.
        for organism in population.iter() {
            ancestry.seed(
                organism.id,
                organism.parent,
                organism.generation,
                organism.birth_tick,
            );
        }
        Ok(ancestry)
    }

    /// An empty index with room for `room` entries, beginning at `since`.
    fn with_room(room: usize, since: Tick) -> Result<Self, CoreError> {
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(room)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "ancestry index",
                bytes: bytes_for::<Entry>(room),
            })?;
        Ok(Self {
            since,
            entries,
            newest: None,
            followed: None,
        })
    }

    /// Take live `organism`, above every identity taken before it, under
    /// `parent` when the index holds it alive.
    fn seed(
        &mut self,
        organism: OrganismId,
        parent: Option<OrganismId>,
        generation: Generation,
        birth_tick: Tick,
    ) {
        let at = self.entries.len();
        let mut entry = Entry::new(organism, parent, generation, birth_tick);
        if let Some(parent) = parent
            && let Some(above) = self.find(parent)
            && self.entries[above].standing == Standing::Living
        {
            entry.link = Some(parent);
            let parent = &mut self.entries[above];
            parent.lines += 1;
            parent.children_living += 1;
        }
        self.entries.push(entry);
        self.newest = Some(organism);
        let mut above = self.up(at);
        while let Some(index) = above {
            self.entries[index].living += 1;
            above = self.up(index);
        }
    }

    /// Whether folding `report` would find every organism it names where
    /// it must be: each birth's parent and each death alive in the index
    /// (or, for a death, born in the same tick), and the newcomers' fresh
    /// identities ascending above every one the index has taken.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] naming the first that is not.
    pub(crate) fn admits(&self, report: &TickReport) -> Result<(), CoreError> {
        self.admits_events(
            report
                .births
                .iter()
                .map(|birth| (birth.parent, birth.child)),
            report.deaths.iter().map(|death| death.id),
            report
                .introductions
                .iter()
                .map(|introduction| introduction.organism),
        )
    }

    /// [`Ancestry::admits`] for one tick's events, as
    /// [`Ancestry::fold_events`] takes them.
    fn admits_events(
        &self,
        births: impl Iterator<Item = (OrganismId, OrganismId)> + Clone,
        deaths: impl Iterator<Item = OrganismId>,
        introductions: impl Iterator<Item = OrganismId>,
    ) -> Result<(), CoreError> {
        let conflict = |detail: String| CoreError::StateConflict {
            operation: "ancestry index",
            detail,
        };
        let living = |organism: OrganismId| {
            self.find(organism)
                .is_some_and(|at| self.entries[at].standing == Standing::Living)
        };
        let before = self.newest;
        let mut newest = before;
        for (parent, child) in births.clone() {
            if !living(parent) {
                return Err(conflict(format!(
                    "{child} was born to {parent}, which the index does not hold alive"
                )));
            }
            if newest.is_some_and(|newest| child <= newest) {
                return Err(conflict(format!(
                    "{child} was born after an identity at or above it"
                )));
            }
            newest = Some(child);
        }
        for organism in deaths {
            // Only a child born in the same tick is above every identity
            // the index took before it.
            let held = if before.is_none_or(|before| organism > before) {
                births.clone().any(|(_, child)| child == organism)
            } else {
                living(organism)
            };
            if !held {
                return Err(conflict(format!(
                    "{organism} died, but the index does not hold it alive"
                )));
            }
        }
        for organism in introductions {
            if newest.is_some_and(|newest| organism <= newest) {
                return Err(conflict(format!(
                    "{organism} was introduced after an identity at or above it"
                )));
            }
            newest = Some(organism);
        }
        Ok(())
    }

    /// Fold `report`, which [`Ancestry::admits`] passed: its births, then
    /// its deaths, then its introductions. It never allocates.
    pub(crate) fn fold(&mut self, report: &TickReport) {
        self.fold_events(
            report.tick,
            report
                .births
                .iter()
                .map(|birth| (birth.parent, birth.child)),
            report.deaths.iter().map(|death| death.id),
            report
                .introductions
                .iter()
                .map(|introduction| introduction.organism),
        );
    }

    /// Fold one published tick `tick`'s events: its births as `(parent,
    /// child)`, then its deaths, then its introductions.
    fn fold_events(
        &mut self,
        tick: Tick,
        births: impl ExactSizeIterator<Item = (OrganismId, OrganismId)>,
        deaths: impl Iterator<Item = OrganismId>,
        introductions: impl ExactSizeIterator<Item = OrganismId>,
    ) {
        self.make_room(births.len());
        for (parent, child) in births {
            self.bear(parent, child, tick);
        }
        // The followed organism's descendants are marked as it dies, while
        // every link below it still reaches it: a later death resolves its
        // own link past a dead ancestor that has passed its place on. Its
        // heir is chosen once every death is folded, and before clearing
        // out for the introductions can take its entry.
        let mut dead_line = None;
        for organism in deaths {
            self.die(organism);
            if let Some(line) = self.followed
                && line.alive
                && line.organism == organism
                && let Some(at) = self.find(organism)
            {
                self.mark_descendants(at);
                dead_line = Some(at);
            }
        }
        if let Some(at) = dead_line {
            self.pass_line(at, tick);
        }
        self.make_room(introductions.len());
        for organism in introductions {
            self.entries
                .push(Entry::new(organism, None, Generation::FOUNDER, tick));
            self.newest = Some(organism);
        }
    }

    /// What the index knows of live `organism`; `None` when it does not
    /// hold it alive.
    pub(crate) fn descendants(&self, organism: OrganismId) -> Option<Descendants> {
        let at = self.find(organism)?;
        let entry = &self.entries[at];
        if entry.standing != Standing::Living {
            return None;
        }
        let mut path = Vec::with_capacity(ANCESTRY_PATH);
        let mut beyond = 0_u64;
        let mut above = self.resolve(entry.link);
        while let Some(index) = above {
            let ancestor = &self.entries[index];
            if path.len() < ANCESTRY_PATH {
                path.push(Ancestor {
                    organism: ancestor.organism,
                    generation: ancestor.generation,
                    birth_tick: ancestor.birth_tick,
                    alive: ancestor.standing == Standing::Living,
                    born: ancestor.born,
                    living: ancestor.living,
                });
            } else {
                beyond += 1;
            }
            above = self.resolve(ancestor.link);
        }
        Some(Descendants {
            since: self.since,
            children_born: entry.children_born,
            children_living: entry.children_living,
            born: entry.born,
            living: entry.living,
            path,
            beyond,
        })
    }

    /// How much of its room it holds.
    pub(crate) fn held(&self) -> AncestryHeld {
        let members = self.entries.iter().filter(|entry| entry.member()).count();
        let dead = self
            .entries
            .iter()
            .filter(|entry| entry.standing == Standing::Meeting)
            .count();
        AncestryHeld {
            since: self.since,
            members,
            dead,
            entries: self.entries.len(),
            room: self.entries.capacity(),
            bytes: self.bytes(),
        }
    }

    /// The bytes its room takes.
    pub(crate) fn bytes(&self) -> u64 {
        capacity_bytes(&self.entries)
    }

    /// Follow live `organism`'s line from committed tick `tick`, or none.
    /// Following an organism begins a line from it, even the one that has
    /// the line now.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when the index does not hold `organism`
    /// alive; the line followed before is kept.
    pub(crate) fn follow(
        &mut self,
        organism: Option<OrganismId>,
        tick: Tick,
    ) -> Result<Option<Followed>, CoreError> {
        let Some(organism) = organism else {
            self.followed = None;
            return Ok(None);
        };
        if !self
            .find(organism)
            .is_some_and(|at| self.entries[at].standing == Standing::Living)
        {
            return Err(CoreError::StateConflict {
                operation: "follow a line",
                detail: format!("the ancestry index does not hold {organism} alive"),
            });
        }
        let line = Followed {
            origin: organism,
            organism,
            since: tick,
            handoffs: 0,
            alive: true,
        };
        self.followed = Some(line);
        Ok(Some(line))
    }

    /// The line it follows, if any.
    pub(crate) const fn followed(&self) -> Option<Followed> {
        self.followed
    }

    /// Mark each entry above entry `at` that descends from it through the
    /// index's links. Ascending, so each link, which names an ancestor and
    /// so a lower identity, is read after it is marked.
    fn mark_descendants(&mut self, at: usize) {
        let organism = self.entries[at].organism;
        for below in at + 1..self.entries.len() {
            let descends = match self.entries[below].link {
                Some(link) if link == organism => true,
                Some(link) if link > organism => self
                    .find(link)
                    .is_some_and(|above| self.entries[above].descends),
                _ => false,
            };
            self.entries[below].descends = descends;
        }
    }

    /// Pass the line from entry `at`, which died in tick `tick` and whose
    /// descendants [`Ancestry::mark_descendants`] marked, to its heir: the
    /// marked entry alive now, fewest generations below and then eldest.
    /// With none, the line ends.
    fn pass_line(&mut self, at: usize, tick: Tick) {
        let heir = self.entries[at + 1..]
            .iter()
            .filter(|entry| entry.descends && entry.standing == Standing::Living)
            .min_by_key(|entry| (entry.generation, entry.organism))
            .map(|entry| entry.organism);
        if let Some(line) = &mut self.followed {
            match heir {
                Some(heir) => {
                    line.organism = heir;
                    line.since = tick;
                    line.handoffs = line.handoffs.saturating_add(1);
                }
                None => line.alive = false,
            }
        }
    }

    fn find(&self, organism: OrganismId) -> Option<usize> {
        self.entries
            .binary_search_by_key(&organism, |entry| entry.organism)
            .ok()
    }

    /// The member `link` stands for, past every ancestor that has passed
    /// its place on.
    fn resolve(&self, mut link: Option<OrganismId>) -> Option<usize> {
        loop {
            let at = self.find(link?)?;
            let entry = &self.entries[at];
            match entry.standing {
                Standing::Living | Standing::Meeting => return Some(at),
                Standing::Passed => link = entry.link,
                Standing::Dropped => return None,
            }
        }
    }

    /// Entry `at`'s nearest ancestor in the index, its link brought up to
    /// date.
    fn up(&mut self, at: usize) -> Option<usize> {
        let above = self.resolve(self.entries[at].link);
        self.entries[at].link = above.map(|index| self.entries[index].organism);
        above
    }

    /// Room for `newcomers` more entries: clear out those that have left
    /// when the room would not hold them otherwise.
    fn make_room(&mut self, newcomers: usize) {
        if self.entries.len().saturating_add(newcomers) <= self.entries.capacity() {
            return;
        }
        // Ascending: an ancestor comes before its descendants, so its own
        // link is already resolved when theirs are.
        for at in 0..self.entries.len() {
            if let Some(link) = self.entries[at].link
                && let Some(above) = self.find(link)
            {
                let Entry { standing, link, .. } = self.entries[above];
                match standing {
                    Standing::Living | Standing::Meeting => {}
                    Standing::Passed => self.entries[at].link = link,
                    Standing::Dropped => self.entries[at].link = None,
                }
            }
        }
        self.entries.retain(Entry::member);
    }

    fn bear(&mut self, parent: OrganismId, child: OrganismId, tick: Tick) {
        let Some(at) = self
            .find(parent)
            .filter(|&at| self.entries[at].standing == Standing::Living)
        else {
            return;
        };
        let generation = {
            let entry = &mut self.entries[at];
            entry.children_born += 1;
            entry.children_living += 1;
            entry.lines += 1;
            entry.generation
        };
        let mut above = Some(at);
        while let Some(index) = above {
            let ancestor = &mut self.entries[index];
            ancestor.born += 1;
            ancestor.living += 1;
            above = self.up(index);
        }
        // The world refused a generation past the largest when it bore it.
        let mut entry = Entry::new(
            child,
            Some(parent),
            generation.child().unwrap_or(generation),
            tick,
        );
        entry.link = Some(parent);
        self.entries.push(entry);
        self.newest = Some(child);
    }

    fn die(&mut self, organism: OrganismId) {
        let Some(at) = self
            .find(organism)
            .filter(|&at| self.entries[at].standing == Standing::Living)
        else {
            return;
        };
        if let Some(parent) = self.entries[at].parent
            && let Some(above) = self.find(parent)
        {
            let parent = &mut self.entries[above];
            parent.children_living = parent.children_living.saturating_sub(1);
        }
        self.entries[at].standing = Standing::Meeting;
        let mut above = self.up(at);
        while let Some(index) = above {
            let ancestor = &mut self.entries[index];
            ancestor.living = ancestor.living.saturating_sub(1);
            above = self.up(index);
        }
        // Leave if fewer than two of its lines live on, and the ancestor it
        // leaves with no line may leave too.
        let mut at = at;
        loop {
            let entry = &self.entries[at];
            if entry.standing != Standing::Meeting {
                return;
            }
            match entry.lines {
                0 => {
                    let above = self.up(at);
                    self.entries[at].standing = Standing::Dropped;
                    let Some(index) = above else {
                        return;
                    };
                    let ancestor = &mut self.entries[index];
                    ancestor.lines = ancestor.lines.saturating_sub(1);
                    at = index;
                }
                1 => {
                    self.up(at);
                    self.entries[at].standing = Standing::Passed;
                    return;
                }
                _ => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! The index against an oracle that keeps every organism a random
    //! genealogy ever had, never pruning, and works out each live
    //! organism's counts and the ancestors where its lines meet by walking
    //! whole parent chains. The room is small, so the index clears out its
    //! leavers many times.

    use std::collections::BTreeMap;

    use super::*;

    /// A seeded draw sequence (`SplitMix64`).
    struct Draws(u64);

    impl Draws {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }

        fn below(&mut self, bound: u64) -> u64 {
            self.next() % bound
        }
    }

    /// One organism the oracle knows.
    #[derive(Debug, Clone, Copy)]
    struct Known {
        /// Its parent when the parent is known: alive at `since` or later.
        parent: Option<OrganismId>,
        generation: Generation,
        birth_tick: Tick,
        /// Born after `since`.
        counted: bool,
        alive: bool,
    }

    /// Every organism alive at `since` or born or introduced after it.
    #[derive(Debug)]
    struct Genealogy {
        since: Tick,
        known: BTreeMap<OrganismId, Known>,
        next: u64,
    }

    impl Genealogy {
        fn live(&self) -> Vec<OrganismId> {
            self.known
                .iter()
                .filter(|(_, known)| known.alive)
                .map(|(&id, _)| id)
                .collect()
        }

        fn issue(&mut self) -> OrganismId {
            let id = OrganismId::new(self.next);
            self.next += 1;
            id
        }

        fn fold(
            &mut self,
            tick: Tick,
            births: &[(OrganismId, OrganismId)],
            deaths: &[OrganismId],
            introductions: &[OrganismId],
        ) {
            for &(parent, child) in births {
                let generation = self.known[&parent].generation.child().expect("generation");
                self.known.insert(
                    child,
                    Known {
                        parent: Some(parent),
                        generation,
                        birth_tick: tick,
                        counted: true,
                        alive: true,
                    },
                );
            }
            for death in deaths {
                self.known.get_mut(death).expect("known").alive = false;
            }
            for &organism in introductions {
                self.known.insert(
                    organism,
                    Known {
                        parent: None,
                        generation: Generation::FOUNDER,
                        birth_tick: tick,
                        counted: true,
                        alive: true,
                    },
                );
            }
        }

        /// Whether `organism` descends from `ancestor` along known parents.
        fn descends(&self, mut organism: OrganismId, ancestor: OrganismId) -> bool {
            while let Some(parent) = self.known[&organism].parent {
                if parent == ancestor {
                    return true;
                }
                organism = parent;
            }
            false
        }

        /// The heir of `organism`: its descendant alive now, fewest
        /// generations below it and then the eldest.
        fn heir(&self, organism: OrganismId) -> Option<OrganismId> {
            self.known
                .iter()
                .filter(|&(&id, known)| known.alive && self.descends(id, organism))
                .min_by_key(|&(&id, known)| (known.generation, id))
                .map(|(&id, _)| id)
        }

        /// The genealogy an index begun now knows: the live organisms,
        /// each under its parent when that parent lives, none counted.
        fn rebased(&self, since: Tick) -> Self {
            let known = self
                .known
                .iter()
                .filter(|(_, known)| known.alive)
                .map(|(&id, known)| {
                    let parent = known
                        .parent
                        .filter(|parent| self.known.get(parent).is_some_and(|known| known.alive));
                    (
                        id,
                        Known {
                            parent,
                            counted: false,
                            ..*known
                        },
                    )
                })
                .collect();
            Self {
                since,
                known,
                next: self.next,
            }
        }

        /// What the index must say of every live organism.
        fn expected(&self) -> BTreeMap<OrganismId, Descendants> {
            let mut children: BTreeMap<OrganismId, Vec<OrganismId>> = BTreeMap::new();
            for (&id, known) in &self.known {
                if let Some(parent) = known.parent {
                    children.entry(parent).or_default().push(id);
                }
            }
            // (born below, living below, lines with live members); a child's
            // identity is above its parent's, so descending order sees
            // children first.
            let mut below: BTreeMap<OrganismId, (u64, u64, u32)> = BTreeMap::new();
            for (&id, _) in self.known.iter().rev() {
                let (mut born, mut living, mut lines) = (0, 0, 0);
                for child in children.get(&id).into_iter().flatten() {
                    let known = &self.known[child];
                    let (child_born, child_living, child_lines) = below[child];
                    born += child_born + u64::from(known.counted);
                    living += child_living + u64::from(known.alive);
                    if known.alive || child_lines > 0 {
                        lines += 1;
                    }
                }
                below.insert(id, (born, living, lines));
            }
            let mut expected = BTreeMap::new();
            for (&organism, known) in &self.known {
                if !known.alive {
                    continue;
                }
                let mine = children.get(&organism).map_or(&[][..], Vec::as_slice);
                let mut ancestors = Vec::new();
                let mut parent = known.parent;
                while let Some(id) = parent {
                    let known = &self.known[&id];
                    let (born, living, lines) = below[&id];
                    if known.alive || lines >= 2 {
                        ancestors.push(Ancestor {
                            organism: id,
                            generation: known.generation,
                            birth_tick: known.birth_tick,
                            alive: known.alive,
                            born,
                            living,
                        });
                    }
                    parent = known.parent;
                }
                let beyond = ancestors.len().saturating_sub(ANCESTRY_PATH) as u64;
                ancestors.truncate(ANCESTRY_PATH);
                let (born, living, _) = below[&organism];
                let count = |keep: fn(&Known) -> bool| {
                    mine.iter().filter(|child| keep(&self.known[child])).count() as u64
                };
                expected.insert(
                    organism,
                    Descendants {
                        since: self.since,
                        children_born: count(|known| known.counted),
                        children_living: count(|known| known.alive),
                        born,
                        living,
                        path: ancestors,
                        beyond,
                    },
                );
            }
            expected
        }
    }

    /// One random tick of a population of at most `capacity`: up to three
    /// births to live parents, half of them to one of the three newest so
    /// that lines grow deep; deaths (the odd identities dying eight times
    /// as often as the even ones, so some lines of live ancestors grow
    /// long, and now and then a newborn) listed in no particular order; and
    /// now and then an introduction.
    fn draw(
        genealogy: &mut Genealogy,
        draws: &mut Draws,
        capacity: usize,
    ) -> (
        Vec<(OrganismId, OrganismId)>,
        Vec<OrganismId>,
        Vec<OrganismId>,
    ) {
        let live = genealogy.live();
        let room = capacity - live.len();
        let mut births = Vec::new();
        if !live.is_empty() {
            for _ in 0..(draws.below(4) as usize).min(room) {
                let newest = live.len().min(3) as u64;
                let parent = if draws.below(2) == 0 {
                    live[live.len() - 1 - draws.below(newest) as usize]
                } else {
                    live[draws.below(live.len() as u64) as usize]
                };
                births.push((parent, genealogy.issue()));
            }
        }
        let mut deaths: Vec<OrganismId> = live
            .iter()
            .copied()
            .filter(|id| draws.below(if id.get() % 2 == 1 { 6 } else { 48 }) == 0)
            .collect();
        for &(_, child) in &births {
            if draws.below(16) == 0 {
                deaths.push(child);
            }
        }
        // No particular order: rotate by a draw.
        if !deaths.is_empty() {
            let turn = draws.below(deaths.len() as u64) as usize;
            deaths.rotate_left(turn);
        }
        let after = live.len() + births.len() - deaths.len();
        let mut introductions = Vec::new();
        if draws.below(12) == 0 || after == 0 {
            for _ in 0..(1 + draws.below(2) as usize).min(capacity - after) {
                introductions.push(genealogy.issue());
            }
        }
        (births, deaths, introductions)
    }

    /// Fold `ticks` random ticks into `index` and `genealogy` alike, each
    /// admitted first, checking every live organism after each; returns
    /// how often it cleared out, the most it held past `ANCESTRY_PATH`, and
    /// whether a path ever passed a dead ancestor.
    fn run(
        index: &mut Ancestry,
        genealogy: &mut Genealogy,
        draws: &mut Draws,
        capacity: usize,
        ticks: u64,
    ) -> (usize, u64, bool) {
        let room = index.entries.capacity();
        let (mut cleared, mut beyond, mut dead_ancestor) = (0, 0, false);
        let start = genealogy
            .known
            .values()
            .map(|known| known.birth_tick.get())
            .max()
            .unwrap_or(0)
            .max(genealogy.since.get());
        for tick in start + 1..=start + ticks {
            let tick = Tick::new(tick);
            let (births, deaths, introductions) = draw(genealogy, draws, capacity);
            index
                .admits_events(
                    births.iter().copied(),
                    deaths.iter().copied(),
                    introductions.iter().copied(),
                )
                .unwrap_or_else(|error| panic!("{tick:?}: {error}"));
            let before = index.entries.len();
            index.fold_events(
                tick,
                births.iter().copied(),
                deaths.iter().copied(),
                introductions.iter().copied(),
            );
            genealogy.fold(tick, &births, &deaths, &introductions);
            if index.entries.len() < before + births.len() + introductions.len() {
                cleared += 1;
            }
            // The room never grows: folding never allocates.
            assert_eq!(index.entries.capacity(), room, "{tick:?}");
            let live = genealogy.live();
            let held = index.held();
            assert_eq!(
                held.members - held.dead,
                live.len(),
                "{tick:?}: members {held:?}"
            );
            assert!(held.dead < live.len().max(1), "{tick:?}: {held:?}");
            let expected = genealogy.expected();
            assert_eq!(expected.len(), live.len());
            for (&id, expected) in &expected {
                let got = index
                    .descendants(id)
                    .expect("a live organism's descendants");
                assert_eq!(&got, expected, "{tick:?}: {id}");
                beyond = beyond.max(got.beyond);
                dead_ancestor |= got.path.iter().any(|ancestor| !ancestor.alive);
            }
            for (&id, known) in &genealogy.known {
                if !known.alive {
                    assert_eq!(index.descendants(id), None, "{tick:?}: dead {id}");
                }
            }
        }
        (cleared, beyond, dead_ancestor)
    }

    fn founded(founders: u64, capacity: usize) -> (Ancestry, Genealogy) {
        let mut index =
            Ancestry::with_room(capacity * ROOM_PER_ORGANISM, Tick::ZERO).expect("room");
        let mut genealogy = Genealogy {
            since: Tick::ZERO,
            known: BTreeMap::new(),
            next: 0,
        };
        for _ in 0..founders {
            let id = genealogy.issue();
            index.seed(id, None, Generation::FOUNDER, Tick::ZERO);
            genealogy.known.insert(
                id,
                Known {
                    parent: None,
                    generation: Generation::FOUNDER,
                    birth_tick: Tick::ZERO,
                    counted: false,
                    alive: true,
                },
            );
        }
        (index, genealogy)
    }

    #[test]
    fn the_index_says_what_the_whole_genealogy_says() {
        for seed in [1_u64, 2, 3, 0x5eed] {
            let capacity = 24;
            let (mut index, mut genealogy) = founded(6, capacity);
            let mut draws = Draws(seed);
            let (cleared, beyond, dead_ancestor) =
                run(&mut index, &mut genealogy, &mut draws, capacity, 3_000);
            // What the run must have exercised.
            assert!(cleared > 20, "seed {seed}: cleared out {cleared} times");
            assert!(beyond > 0, "seed {seed}: no path past {ANCESTRY_PATH}");
            assert!(dead_ancestor, "seed {seed}: no dead ancestor on a path");
        }
    }

    #[test]
    fn an_index_begun_later_starts_from_the_live_population() {
        for seed in [7_u64, 8] {
            let capacity = 24;
            let (mut first, mut genealogy) = founded(6, capacity);
            let mut draws = Draws(seed);
            run(&mut first, &mut genealogy, &mut draws, capacity, 400);
            let since = Tick::new(400);
            let mut later = genealogy.rebased(since);
            // Some live organism's parent died before the index began.
            assert!(
                later
                    .known
                    .values()
                    .any(|known| known.parent.is_none() && known.generation != Generation::FOUNDER),
                "seed {seed}: every live organism's parent lives"
            );
            let mut index = Ancestry::with_room(capacity * ROOM_PER_ORGANISM, since).expect("room");
            for &id in later.known.keys() {
                // Its row's parent, alive or not: the index links it only
                // to a live one.
                let row = &genealogy.known[&id];
                index.seed(id, row.parent, row.generation, row.birth_tick);
            }
            run(&mut index, &mut later, &mut draws, capacity, 1_500);
        }
    }

    #[test]
    fn events_the_index_cannot_hold_are_refused() {
        let (mut index, _) = founded(3, 8);
        let [a, b, c] = [0, 1, 2].map(OrganismId::new);
        index.fold_events(
            Tick::new(1),
            [(a, OrganismId::new(3))].into_iter(),
            [b].into_iter(),
            [].into_iter(),
        );
        let child = OrganismId::new(4);
        let refused = |births: &[(OrganismId, OrganismId)],
                       deaths: &[OrganismId],
                       introductions: &[OrganismId]| {
            index
                .admits_events(
                    births.iter().copied(),
                    deaths.iter().copied(),
                    introductions.iter().copied(),
                )
                .is_err()
        };
        // A parent the index does not hold, or holds dead.
        assert!(refused(&[(OrganismId::new(99), child)], &[], &[]));
        assert!(refused(&[(b, child)], &[], &[]));
        // A child or an introduction at or below an identity it took.
        assert!(refused(&[(a, OrganismId::new(3))], &[], &[]));
        assert!(refused(&[], &[], &[OrganismId::new(2)]));
        assert!(refused(&[(a, child)], &[], &[child]));
        // A death it does not hold alive: unknown, already dead, or above
        // every identity and not born in the tick.
        assert!(refused(&[], &[OrganismId::new(42)], &[]));
        assert!(refused(&[], &[b], &[]));
        assert!(refused(&[(a, child)], &[OrganismId::new(5)], &[]));
        // What it can hold: a birth, its child dying in the same tick, a
        // death, an introduction above them.
        assert!(!refused(&[(a, child)], &[child, c], &[OrganismId::new(5)]));
    }

    /// What a run of followed lines saw.
    #[derive(Debug, Default)]
    struct Lines {
        /// Lines passed to an heir.
        handoffs: usize,
        /// Lines ended with no descendant alive.
        ended: usize,
        /// Heirs more than one generation below: no child was alive.
        deeper: usize,
        /// Heirs with a younger generation's descendant alive and elder
        /// than them, which the rule passed over.
        nearer_not_elder: usize,
    }

    /// Fold `ticks` random ticks into `index` and `genealogy` alike,
    /// following a live organism's line whenever none is followed alive,
    /// and check the line after each against the oracle's heir.
    fn follow_run(
        index: &mut Ancestry,
        genealogy: &mut Genealogy,
        draws: &mut Draws,
        capacity: usize,
        ticks: u64,
    ) -> Lines {
        let mut lines = Lines::default();
        let start = genealogy
            .known
            .values()
            .map(|known| known.birth_tick.get())
            .max()
            .unwrap_or(0)
            .max(genealogy.since.get());
        for tick in start + 1..=start + ticks {
            let committed = Tick::new(tick - 1);
            let tick = Tick::new(tick);
            if index.followed().is_none_or(|line| !line.alive) {
                let live = genealogy.live();
                let pick = live[draws.below(live.len() as u64) as usize];
                let line = index.follow(Some(pick), committed).expect("follows");
                assert_eq!(
                    line,
                    Some(Followed {
                        origin: pick,
                        organism: pick,
                        since: committed,
                        handoffs: 0,
                        alive: true,
                    })
                );
            }
            let before = index.followed().expect("a line");
            let (births, deaths, introductions) = draw(genealogy, draws, capacity);
            index
                .admits_events(
                    births.iter().copied(),
                    deaths.iter().copied(),
                    introductions.iter().copied(),
                )
                .unwrap_or_else(|error| panic!("{tick:?}: {error}"));
            index.fold_events(
                tick,
                births.iter().copied(),
                deaths.iter().copied(),
                introductions.iter().copied(),
            );
            genealogy.fold(tick, &births, &deaths, &introductions);
            let line = index.followed().expect("a line");
            if !deaths.contains(&before.organism) {
                assert_eq!(line, before, "{tick:?}");
                continue;
            }
            match genealogy.heir(before.organism) {
                Some(heir) => {
                    assert_eq!(
                        line,
                        Followed {
                            organism: heir,
                            since: tick,
                            handoffs: before.handoffs + 1,
                            ..before
                        },
                        "{tick:?}"
                    );
                    lines.handoffs += 1;
                    let generation = genealogy.known[&heir].generation;
                    let dead = genealogy.known[&before.organism].generation;
                    if generation > dead.child().expect("a generation") {
                        lines.deeper += 1;
                    }
                    if genealogy.known.iter().any(|(&id, known)| {
                        known.alive
                            && id < heir
                            && known.generation > generation
                            && genealogy.descends(id, before.organism)
                    }) {
                        lines.nearer_not_elder += 1;
                    }
                }
                None => {
                    assert_eq!(
                        line,
                        Followed {
                            alive: false,
                            ..before
                        },
                        "{tick:?}"
                    );
                    lines.ended += 1;
                }
            }
        }
        lines
    }

    #[test]
    fn a_followed_line_passes_to_the_nearest_eldest_living_descendant() {
        for seed in [11_u64, 12, 13, 0xf011] {
            let capacity = 24;
            let (mut index, mut genealogy) = founded(6, capacity);
            let mut draws = Draws(seed);
            let room = index.entries.capacity();
            let lines = follow_run(&mut index, &mut genealogy, &mut draws, capacity, 3_000);
            // Following never allocates either.
            assert_eq!(index.entries.capacity(), room);
            // What the runs must have exercised: lines passed on and
            // ended, an heir past a dead child, and the generation rule
            // overriding age.
            assert!(lines.handoffs > 30, "seed {seed}: {lines:?}");
            assert!(lines.ended > 10, "seed {seed}: {lines:?}");
            assert!(lines.deeper > 0, "seed {seed}: {lines:?}");
            assert!(lines.nearer_not_elder > 0, "seed {seed}: {lines:?}");
        }
    }

    #[test]
    fn a_line_followed_in_an_index_begun_later_passes_through_its_lines() {
        for seed in [17_u64, 18] {
            let capacity = 24;
            let (mut first, mut genealogy) = founded(6, capacity);
            let mut draws = Draws(seed);
            run(&mut first, &mut genealogy, &mut draws, capacity, 400);
            let since = Tick::new(400);
            let mut later = genealogy.rebased(since);
            let mut index = Ancestry::with_room(capacity * ROOM_PER_ORGANISM, since).expect("room");
            for &id in later.known.keys() {
                let row = &genealogy.known[&id];
                index.seed(id, row.parent, row.generation, row.birth_tick);
            }
            let lines = follow_run(&mut index, &mut later, &mut draws, capacity, 1_500);
            assert!(
                lines.handoffs >= 10 && lines.ended > 0 && lines.deeper > 0,
                "seed {seed}: {lines:?}"
            );
        }
    }

    #[test]
    fn only_a_live_organism_is_followed() {
        let (mut index, _) = founded(3, 8);
        let [a, b] = [0, 1].map(OrganismId::new);
        let line = index.follow(Some(a), Tick::ZERO).expect("follows a");
        index.fold_events(
            Tick::new(1),
            [(a, OrganismId::new(3))].into_iter(),
            [b].into_iter(),
            [].into_iter(),
        );
        // Dead, unknown: refused, and the line kept.
        for organism in [b, OrganismId::new(42)] {
            assert!(index.follow(Some(organism), Tick::new(1)).is_err());
            assert_eq!(index.followed(), line);
        }
        // Following again begins a line, from the organism followed.
        let child = OrganismId::new(3);
        let again = index.follow(Some(child), Tick::new(1)).expect("follows");
        assert_eq!(
            again,
            Some(Followed {
                origin: child,
                organism: child,
                since: Tick::new(1),
                handoffs: 0,
                alive: true,
            })
        );
        // None follows nothing.
        assert_eq!(index.follow(None, Tick::new(1)).expect("none"), None);
        assert_eq!(index.followed(), None);
    }

    /// The followed organism dies in the same tick as its eldest child,
    /// with the deaths listed either way round: the heir is its other
    /// child, alive once every death is folded, not the dead child's own
    /// child, a generation further below.
    #[test]
    fn the_heir_is_chosen_once_the_ticks_deaths_are_folded() {
        let ids = |raw: [u64; 4]| raw.map(OrganismId::new);
        for reversed in [false, true] {
            let (mut index, _) = founded(2, 16);
            let [a, _, eldest, younger] = ids([0, 1, 2, 3]);
            let grandchild = OrganismId::new(4);
            index.fold_events(
                Tick::new(1),
                [(a, eldest), (a, younger)].into_iter(),
                [].into_iter(),
                [].into_iter(),
            );
            index.fold_events(
                Tick::new(2),
                [(eldest, grandchild)].into_iter(),
                [].into_iter(),
                [].into_iter(),
            );
            index.follow(Some(a), Tick::new(2)).expect("follows");
            let deaths = if reversed { [eldest, a] } else { [a, eldest] };
            index.fold_events(
                Tick::new(3),
                [].into_iter(),
                deaths.into_iter(),
                [].into_iter(),
            );
            assert_eq!(
                index.followed(),
                Some(Followed {
                    origin: a,
                    organism: younger,
                    since: Tick::new(3),
                    handoffs: 1,
                    alive: true,
                }),
                "reversed {reversed}"
            );
        }
    }

    /// The followed organism bears its only child in the tick it dies:
    /// that newborn is its heir. Its own line then ends when it dies
    /// childless.
    #[test]
    fn a_child_born_in_the_tick_its_parent_dies_is_its_heir() {
        let (mut index, _) = founded(2, 16);
        let [a, newborn] = [0, 2].map(OrganismId::new);
        index.follow(Some(a), Tick::ZERO).expect("follows");
        index.fold_events(
            Tick::new(1),
            [(a, newborn)].into_iter(),
            [a].into_iter(),
            [].into_iter(),
        );
        let passed = Followed {
            origin: a,
            organism: newborn,
            since: Tick::new(1),
            handoffs: 1,
            alive: true,
        };
        assert_eq!(index.followed(), Some(passed));
        index.fold_events(
            Tick::new(2),
            [].into_iter(),
            [newborn].into_iter(),
            [].into_iter(),
        );
        assert_eq!(
            index.followed(),
            Some(Followed {
                alive: false,
                ..passed
            })
        );
        // An ended line stays as it ended through later ticks.
        index.fold_events(
            Tick::new(3),
            [].into_iter(),
            [OrganismId::new(1)].into_iter(),
            [OrganismId::new(3)].into_iter(),
        );
        assert_eq!(
            index.followed(),
            Some(Followed {
                alive: false,
                ..passed
            })
        );
    }
}
