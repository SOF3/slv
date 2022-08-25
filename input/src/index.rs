use std::cmp;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use parking_lot::RwLock;
use slv_proto::{Entry, FieldCondition, IndexMethod, JsonEntry, MessageId};

pub struct Store {
    buffer:    RwLock<MessageBuffer>,
    raw_index: RwLock<VecDeque<MessageId>>,
    indices:   RwLock<HashMap<IndexMethod, Arc<RwLock<Index>>>>,
}

impl Store {
    pub fn new(options: Options) -> Self {
        Self {
            buffer:    RwLock::new(MessageBuffer::new(options.buffer_size)),
            raw_index: Default::default(),
            indices:   Default::default(),
        }
    }

    pub fn push(&self, message: Entry) {
        let target = self.index_target(&message);

        let push_result = {
            let mut buffer = self.buffer.write();
            buffer.push(message)
        };

        self.add_to_index(push_result.added, target);
        if let Some((removed_id, removed_message)) = push_result.removed {
            self.remove_from_index(removed_id, removed_message);
        }
    }

    fn index_target(&self, message: &Entry) -> IndexTarget {
        match message {
            Entry::Raw(_) => IndexTarget::Raw,
            Entry::Json(message) => {
                // indices is only write-locked when a client requests a new index,
                // which is relatively rare.
                // little performance impact is expected from read-locking this field.
                let matched = {
                    let indices = self.indices.read();
                    indices
                        .iter()
                        .filter(|(method, _)| should_index(method, message))
                        .map(|(_, list)| Arc::clone(list))
                        .collect()
                };

                IndexTarget::Json { matched }
            }
        }
    }

    fn add_to_index(&self, id: MessageId, target: IndexTarget) {
        match target {
            IndexTarget::Raw => {
                let mut raw_index = self.raw_index.write();
                raw_index.push_back(id);
            }
            IndexTarget::Json { matched } => {
                for index in matched {
                    let mut index = index.write();
                    index.add(id);
                }
            }
        }
    }

    fn remove_from_index(&self, id: MessageId, message: Entry) {
        match self.index_target(&message) {
            IndexTarget::Raw => {
                let mut index = self.raw_index.write();
                assert_eq!(index.get(0), Some(&id), "raw index inconsistency");
                index.pop_front();
            }
            IndexTarget::Json { matched } => {
                for index in matched {
                    let mut index = index.write();
                    index.remove(id);
                }
            }
        }
    }

    pub fn list_indices(&self) -> Vec<IndexMethod> {
        let indices = self.indices.read();
        indices.keys().cloned().collect()
    }
}

enum IndexTarget {
    Raw,
    Json { matched: Vec<Arc<RwLock<Index>>> },
}

struct MessageBuffer {
    start_index: MessageId,
    bound:       usize,
    deque:       VecDeque<Entry>,
}

impl MessageBuffer {
    fn new(bound: usize) -> Self {
        Self { start_index: MessageId(0), bound, deque: VecDeque::new() }
    }

    fn push(&mut self, message: Entry) -> PushResult {
        assert!(self.deque.len() <= self.bound);

        let removed = if self.deque.len() == self.bound {
            let removed = self.start_index;
            self.start_index.0 += 1;
            let old_message = self.deque.pop_front().expect("bound > 0");
            Some((removed, old_message))
        } else {
            None
        };

        let added = MessageId(self.start_index.0 + self.deque.len());
        self.deque.push_back(message);

        PushResult { added, removed }
    }
}

struct PushResult {
    added:   MessageId,
    removed: Option<(MessageId, Entry)>,
}

struct Index {
    queue: VecDeque<MessageId>,
}

impl Index {
    fn add(&mut self, id: MessageId) {
        if let Some(&last) = self.queue.back() {
            assert!(last < id);
        }
        self.queue.push_back(id);
    }

    fn remove(&mut self, id: MessageId) {
        match self.queue.front() {
            // index did not exist when id was created
            Some(&front) if front > id => {}
            None => {}

            Some(&front) => {
                assert!(front == id, "index contains obsolete message");
                self.queue.pop_front();
            }
        }
    }
}

fn should_index(method: &IndexMethod, message: &JsonEntry) -> bool {
    let mut fields = &message.0[..];
    for unit in &*method.conditions {
        let unit_key = unit.key();

        loop {
            // loop over all fields in the message in order
            let (field_key, field_value) = match fields.split_first() {
                Some((first, rest)) => {
                    fields = rest;
                    first
                }
                None => return false, // no entries named `unit_key`
            };

            match field_key.as_str().cmp(unit_key) {
                cmp::Ordering::Less => continue, // this field is not in the index key
                cmp::Ordering::Greater => return false, // no entries named `unit_key`
                cmp::Ordering::Equal => match unit {
                    FieldCondition::HasKey(_) => {}
                    FieldCondition::KeyValue(_, value) => {
                        if field_value != value {
                            return false;
                        }
                    }
                },
            }
        }
    }

    true
}

#[derive(clap::Parser)]
pub struct Options {
    /// Maximum number of messages to buffer.
    ///
    /// The oldest messages that exceed the buffer are discarded.
    #[clap(long, value_parser)]
    pub buffer_size: usize,
}
