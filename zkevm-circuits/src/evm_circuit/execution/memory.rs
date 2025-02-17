use crate::{
    evm_circuit::{
        execution::{
            bus_mapping_tmp::{Block, Call, ExecStep, Transaction},
            ExecutionGadget,
        },
        param::MAX_GAS_SIZE_IN_BYTES,
        step::ExecutionState,
        util::{
            common_gadget::SameContextGadget,
            constraint_builder::{
                ConstraintBuilder, StepStateTransition,
                Transition::{Delta, To},
            },
            from_bytes,
            math_gadget::IsEqualGadget,
            memory_gadget::MemoryExpansionGadget,
            select, MemoryAddress, Word,
        },
    },
    util::Expr,
};
use bus_mapping::{eth_types::ToLittleEndian, evm::OpcodeId};
use halo2::{arithmetic::FieldExt, circuit::Region, plonk::Error};
use std::convert::TryInto;

#[derive(Clone, Debug)]
pub(crate) struct MemoryGadget<F> {
    same_context: SameContextGadget<F>,
    address: MemoryAddress<F>,
    value: Word<F>,
    memory_expansion: MemoryExpansionGadget<F, MAX_GAS_SIZE_IN_BYTES>,
    is_mload: IsEqualGadget<F>,
    is_mstore8: IsEqualGadget<F>,
}

impl<F: FieldExt> ExecutionGadget<F> for MemoryGadget<F> {
    const NAME: &'static str = "MEMORY";

    const EXECUTION_STATE: ExecutionState = ExecutionState::MEMORY;

    fn configure(cb: &mut ConstraintBuilder<F>) -> Self {
        let opcode = cb.query_cell();

        // In successful case the address must be in 5 bytes
        let address = MemoryAddress::new(cb.query_bytes(), cb.randomness());
        let value = cb.query_word();

        // Check if this is an MLOAD
        let is_mload =
            IsEqualGadget::construct(cb, opcode.expr(), OpcodeId::MLOAD.expr());
        // Check if this is an MSTORE8
        let is_mstore8 = IsEqualGadget::construct(
            cb,
            opcode.expr(),
            OpcodeId::MSTORE8.expr(),
        );
        // This is an MSTORE/MSTORE8
        let is_store = 1.expr() - is_mload.expr();
        // This in an MSTORE/MLOAD
        let is_not_mstore8 = 1.expr() - is_mstore8.expr();

        // Calculate the next memory size and the gas cost for this memory
        // access
        let memory_expansion = MemoryExpansionGadget::construct(
            cb,
            cb.curr.state.memory_size.expr(),
            from_bytes::expr(&address.cells)
                + 1.expr()
                + (is_not_mstore8.clone() * 31.expr()),
        );

        /* Stack operations */
        // Pop the address from the stack
        cb.stack_pop(address.expr());
        // For MLOAD push the value to the stack
        // FOR MSTORE pop the value from the stack
        cb.stack_lookup(
            is_mload.expr(),
            cb.stack_pointer_offset().expr() - is_mload.expr(),
            value.expr(),
        );

        /* Memory operations */
        // Read/Write the value from memory at the specified address
        // We always read/write 32 bytes, but for MSTORE8 this will be
        // 32 lookups for the same LSB byte (at the same gc).
        for idx in 0..32 {
            // For MSTORE8 we write the LSB of value 32x times to the same
            // address For MLOAD and MSTORE we read/write all the
            // bytes of value at an increasing address value.
            let byte = if idx == 31 {
                value.cells[0].expr()
            } else {
                select::expr(
                    is_mstore8.expr(),
                    value.cells[0].expr(),
                    value.cells[31 - idx].expr(),
                )
            };

            // We only increase the offset for MLOAD and MSTORE so that for
            // MSTORE8 `gc` and `address` remain the same.
            let offset = if idx == 0 {
                0.expr()
            } else {
                is_not_mstore8.clone() * idx.expr()
            };
            cb.memory_lookup_with_counter(
                cb.curr.state.rw_counter.expr()
                    + cb.rw_counter_offset().expr()
                    + offset.clone(),
                is_store.clone(),
                from_bytes::expr(&address.cells) + offset,
                byte,
            );
        }

        // State transition
        // - `rw_counter` needs to be increased by 34 when is_not_mstore8,
        //   otherwise to be increased by 31
        // - `program_counter` needs to be increased by 1
        // - `stack_pointer` needs to be increased by 2 when is_store, otherwise
        //   to be same
        // - `memory_size` needs to be set to `next_memory_size`
        let step_state_transition = StepStateTransition {
            rw_counter: Delta(34.expr() - is_mstore8.expr() * 31.expr()),
            program_counter: Delta(1.expr()),
            stack_pointer: Delta(is_store * 2.expr()),
            memory_size: To(memory_expansion.next_memory_size()),
            ..Default::default()
        };
        let same_context = SameContextGadget::construct(
            cb,
            opcode,
            step_state_transition,
            Some(memory_expansion.gas_cost()),
        );

        Self {
            same_context,
            address,
            value,
            memory_expansion,
            is_mload,
            is_mstore8,
        }
    }

    fn assign_exec_step(
        &self,
        region: &mut Region<'_, F>,
        offset: usize,
        block: &Block<F>,
        _: &Transaction<F>,
        _: &Call<F>,
        step: &ExecStep,
    ) -> Result<(), Error> {
        self.same_context.assign_exec_step(region, offset, step)?;

        let opcode = step.opcode.unwrap();

        // Inputs/Outputs
        let [address, value] = [step.rw_indices[0], step.rw_indices[1]]
            .map(|idx| block.rws[idx].stack_value());
        self.address.assign(
            region,
            offset,
            Some(address.to_le_bytes()[..5].try_into().unwrap()),
        )?;
        self.value
            .assign(region, offset, Some(value.to_le_bytes()))?;

        // Check if this is an MLOAD
        self.is_mload.assign(
            region,
            offset,
            F::from(opcode.as_u64()),
            F::from(OpcodeId::MLOAD.as_u64()),
        )?;
        // Check if this is an MSTORE8
        let is_mstore8 = self.is_mstore8.assign(
            region,
            offset,
            F::from(opcode.as_u64()),
            F::from(OpcodeId::MSTORE8.as_u64()),
        )?;

        // Memory expansion
        self.memory_expansion.assign(
            region,
            offset,
            step.memory_size,
            address.as_u64() + 1 + if is_mstore8 == F::one() { 0 } else { 31 },
        )?;

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::evm_circuit::{
        execution::bus_mapping_tmp::{
            Block, Bytecode, Call, ExecStep, Rw, Transaction,
        },
        step::ExecutionState,
        test::{rand_word, run_test_circuit_incomplete_fixed_table},
        util::RandomLinearCombination,
    };
    use bus_mapping::{
        eth_types::{ToBigEndian, ToLittleEndian, Word},
        evm::{GasCost, OpcodeId},
    };
    use halo2::arithmetic::BaseExt;
    use pairing::bn256::Fr as Fp;
    use std::iter;

    fn test_ok(
        opcode: OpcodeId,
        address: Word,
        value: Word,
        memory_size: u64,
        gas_cost: u64,
    ) {
        let is_mload = opcode == OpcodeId::MLOAD;
        let is_mstore8 = opcode == OpcodeId::MSTORE8;

        let randomness = Fp::rand();
        let bytecode = Bytecode::new(
            [
                vec![OpcodeId::PUSH32.as_u8()],
                value.to_be_bytes().to_vec(),
                vec![OpcodeId::PUSH32.as_u8()],
                address.to_be_bytes().to_vec(),
                vec![opcode.as_u8(), OpcodeId::STOP.as_u8()],
            ]
            .concat(),
        );
        let block = Block {
            randomness,
            txs: vec![Transaction {
                calls: vec![Call {
                    id: 1,
                    is_root: false,
                    is_create: false,
                    opcode_source:
                        RandomLinearCombination::random_linear_combine(
                            bytecode.hash.to_le_bytes(),
                            randomness,
                        ),
                }],
                steps: vec![
                    ExecStep {
                        rw_indices: vec![0, 1],
                        execution_state: ExecutionState::MEMORY,
                        rw_counter: 1,
                        program_counter: 66,
                        stack_pointer: 1022,
                        gas_left: gas_cost,
                        gas_cost,
                        memory_size: 0,
                        opcode: Some(opcode),
                        ..Default::default()
                    },
                    ExecStep {
                        execution_state: ExecutionState::STOP,
                        rw_counter: 35 - 31 * is_mstore8 as usize,
                        program_counter: 67,
                        stack_pointer: 1022 + 2 * !is_mload as usize,
                        gas_left: 0,
                        memory_size,
                        opcode: Some(OpcodeId::STOP),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            rws: [
                vec![
                    Rw::Stack {
                        rw_counter: 1,
                        is_write: false,
                        call_id: 1,
                        stack_pointer: 1022,
                        value: address,
                    },
                    Rw::Stack {
                        rw_counter: 2,
                        is_write: is_mload,
                        call_id: 1,
                        stack_pointer: 1022 + !is_mload as usize,
                        value,
                    },
                ],
                (if is_mstore8 {
                    vec![value.to_le_bytes()[0]]
                } else {
                    value.to_be_bytes().to_vec()
                })
                .into_iter()
                .enumerate()
                .map(|(idx, byte)| Rw::Memory {
                    rw_counter: 3 + idx,
                    is_write: !is_mload,
                    call_id: 1,
                    memory_address: (address + idx).as_u64(),
                    byte,
                })
                .collect::<Vec<_>>(),
            ]
            .concat(),
            bytecodes: vec![bytecode],
        };
        assert_eq!(run_test_circuit_incomplete_fixed_table(block), Ok(()));
    }

    #[test]
    fn memory_gadget_simple() {
        test_ok(
            OpcodeId::MSTORE,
            Word::from(0x12FFFF),
            Word::from_big_endian(&(1..33).collect::<Vec<_>>()),
            38913,
            3074206,
        );
        test_ok(
            OpcodeId::MLOAD,
            Word::from(0x12FFFF),
            Word::from_big_endian(&(1..33).collect::<Vec<_>>()),
            38913,
            3074206,
        );
        test_ok(
            OpcodeId::MLOAD,
            Word::from(0x12FFFF) + 16,
            Word::from_big_endian(
                &(17..33).chain(iter::repeat(0).take(16)).collect::<Vec<_>>(),
            ),
            38914,
            3074361,
        );
        test_ok(
            OpcodeId::MSTORE8,
            Word::from(0x12FFFF),
            Word::from_big_endian(&(1..33).collect::<Vec<_>>()),
            38912,
            3074051,
        );
    }

    #[test]
    fn memory_gadget_rand() {
        let calc_memory_size_and_gas_cost = |opcode, address: Word| {
            let memory_size = (address.as_u64()
                + match opcode {
                    OpcodeId::MSTORE | OpcodeId::MLOAD => 32,
                    OpcodeId::MSTORE8 => 1,
                    _ => 0,
                }
                + 31)
                / 32;
            let gas_cost = memory_size * memory_size / 512
                + 3 * memory_size
                + GasCost::FASTEST.as_u64();
            (memory_size, gas_cost)
        };

        for opcode in [OpcodeId::MSTORE, OpcodeId::MLOAD, OpcodeId::MSTORE8] {
            let address = rand_word() % (1u64 << 37);
            let value = rand_word();
            let (memory_size, gas_cost) =
                calc_memory_size_and_gas_cost(opcode, address);
            test_ok(opcode, address, value, memory_size, gas_cost);
        }
    }
}
