#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LookupSlot {
    pub(crate) domain_index: u32,
    pub(crate) attempts: u8,
    pub(crate) resolver_index: u16,
}

#[derive(Debug)]
pub(crate) struct LookupSlab {
    slots: Vec<Option<LookupSlot>>,
    active: usize,
}

impl LookupSlab {
    pub(crate) fn new() -> Self {
        Self {
            slots: vec![None; u16::MAX as usize + 1],
            active: 0,
        }
    }

    pub(crate) fn insert(&mut self, slot: LookupSlot) -> u16 {
        loop {
            let id = rand::random::<u16>();
            let entry = &mut self.slots[id as usize];
            if entry.is_none() {
                *entry = Some(slot);
                self.active += 1;
                return id;
            }
        }
    }

    pub(crate) fn get(&self, id: u16) -> Option<LookupSlot> {
        self.slots[id as usize]
    }

    pub(crate) fn remove(&mut self, id: u16) -> Option<LookupSlot> {
        let slot = self.slots[id as usize].take();
        if slot.is_some() {
            self.active -= 1;
        }
        slot
    }

    pub(crate) fn active_count(&self) -> usize {
        self.active
    }
}
