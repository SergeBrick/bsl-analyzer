use uuid::Uuid;

/// A command (`Команда` of an object or `ОбщаяКоманда`): the owner of a command
/// module. Like a form, a command is a metadata object with its own support
/// record, so a command module is judged by the command itself, never by the
/// object the command hangs off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    name: String,
    uuid: Uuid,
}

impl Command {
    pub fn new(name: impl Into<String>, uuid: Uuid) -> Self {
        Self { name: name.into(), uuid }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The uuid of the metadata object itself: the key the vendor-support map
    /// knows this command by. A command without one is unidentified.
    pub fn uuid(&self) -> Uuid {
        self.uuid
    }

    /// Heap bytes owned by this command: its name string.
    pub fn estimated_heap_size(&self) -> usize {
        self.name.capacity()
    }
}
