use super::{MemoryOp, Op, OpEnum, Operation, StackOp, StorageOp, Target};
use crate::exec_trace::OperationRef;
use itertools::Itertools;

/// The `OperationContainer` is meant to store all of the [`Operation`]s that an
/// [`ExecStep`](crate::circuit_input_builder::ExecStep) performs during its
/// execution.
///
/// Once an operation is inserted into the container, it returns an
/// [`OperationRef`] which holds an index to the operation just inserted.
/// These references are stored inside of the bus-mapping instances of each
/// [`ExecStep`](crate::circuit_input_builder::ExecStep).
///
/// Finally, the container also provides the capability of retrieving all of the
/// `Stack`, `Memory` or `Storage` operations ordered according to the criterias
/// they have specified.
/// That serves as a way to get an input with which is easy to work with in
/// order to construct the State proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationContainer {
    pub(crate) memory: Vec<Operation<MemoryOp>>,
    pub(crate) stack: Vec<Operation<StackOp>>,
    pub(crate) storage: Vec<Operation<StorageOp>>,
}

impl Default for OperationContainer {
    fn default() -> Self {
        Self::new()
    }
}

// TODO: impl Index for OperationContainer
impl OperationContainer {
    /// Generates a new instance of an `OperationContainer`.
    pub fn new() -> Self {
        Self {
            memory: Vec::new(),
            stack: Vec::new(),
            storage: Vec::new(),
        }
    }

    /// Inserts an [`Operation`] into the  container returning a lightwheight
    /// reference to it in the form of an [`OperationRef`] which points to the
    /// location of the inserted operation inside the corresponding container
    /// vector.
    pub fn insert<T: Op>(&mut self, op: Operation<T>) -> OperationRef {
        let gc = op.gc();
        match op.op.into_enum() {
            OpEnum::Memory(op) => {
                self.memory.push(Operation::new(gc, op));
                OperationRef::from((Target::Memory, self.memory.len()))
            }
            OpEnum::Stack(op) => {
                self.stack.push(Operation::new(gc, op));
                OperationRef::from((Target::Stack, self.stack.len()))
            }
            OpEnum::Storage(op) => {
                self.storage.push(Operation::new(gc, op));
                OperationRef::from((Target::Storage, self.storage.len()))
            }
        }
    }

    /// Returns a sorted vector of all of the [`MemoryOp`]s contained inside of
    /// the container.
    pub fn sorted_memory(&self) -> Vec<Operation<MemoryOp>> {
        self.memory.iter().sorted().cloned().collect()
    }

    /// Returns a sorted vector of all of the [`StackOp`]s contained inside of
    /// the container.
    pub fn sorted_stack(&self) -> Vec<Operation<StackOp>> {
        self.stack.iter().sorted().cloned().collect()
    }

    /// Returns a sorted vector of all of the [`StorageOp`]s contained inside of
    /// the container.
    pub fn sorted_storage(&self) -> Vec<Operation<StorageOp>> {
        self.storage.iter().sorted().cloned().collect()
    }
}

#[cfg(test)]
mod container_test {
    use super::*;

    use crate::{
        eth_types::{Address, Word},
        evm::{GlobalCounter, MemoryAddress, StackAddress},
        operation::RW,
    };

    #[test]
    fn operation_container_test() {
        let mut global_counter = GlobalCounter::default();
        let mut operation_container = OperationContainer::default();
        let stack_operation = Operation::new(
            global_counter.inc_pre(),
            StackOp::new(RW::WRITE, StackAddress(1023), Word::from(0x100)),
        );
        let memory_operation = Operation::new(
            global_counter.inc_pre(),
            MemoryOp::new(RW::WRITE, MemoryAddress::from(1), 1),
        );
        let storage_operation = Operation::new(
            global_counter.inc_pre(),
            StorageOp::new(
                RW::WRITE,
                Address::zero(),
                Word::default(),
                Word::from(0x1),
                Word::default(),
            ),
        );
        let stack_ref = operation_container.insert(stack_operation.clone());
        let memory_ref = operation_container.insert(memory_operation.clone());
        let storage_ref = operation_container.insert(storage_operation.clone());

        assert_eq!(operation_container.sorted_stack()[0], stack_operation);
        assert_eq!(operation_container.sorted_memory()[0], memory_operation);
        assert_eq!(operation_container.sorted_storage()[0], storage_operation);
        assert_eq!(stack_ref, OperationRef::from((Target::Stack, 1)));
        assert_eq!(memory_ref, OperationRef::from((Target::Memory, 1)));
        assert_eq!(storage_ref, OperationRef::from((Target::Storage, 1)));
    }
}
