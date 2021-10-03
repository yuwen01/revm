use crate::collection::vec::Vec;
use crate::{opcode::eval, ExitError};
use bytes::Bytes;
use core::{cmp::max, ops::Range};
use primitive_types::U256;

use super::{contract::Contract, memory::Memory, stack::Stack};
use crate::{
    error::{ExitReason, ExitSucceed},
    opcode::{Control, OpCode},
    spec::Spec,
    ExtHandler,
};

pub struct Machine {
    /// Contract information and invoking data
    pub contract: Contract,
    /// Program counter.
    program_counter: usize,
    /// Return value.
    pub return_range: Range<U256>,
    /// Memory.
    pub memory: Memory,
    /// Stack.
    pub stack: Stack,
    /// Return stuff
    pub status: Result<(), ExitReason>,
    pub return_data_buffer: Bytes,
    /// left gas. Memory gas can be found in Memory field.
    pub gas: Gas,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Gas {
    limit: u64,
    used: u64,
    memory: u64,
    refunded: i64,
}
impl Gas {
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            used: 0,
            memory: 0,
            refunded: 0,
        }
    }

    pub fn limit_mut(&mut self) -> &mut u64{
        &mut self.limit
    }

    pub fn limit(&self) -> u64 {
        self.limit
    }
    pub fn used(&self) -> u64 {
        self.used
    }
    pub fn memory(&self) -> u64 {
        self.memory
    }

    pub fn refunded(&self) -> i64 {
        self.refunded
    }

    pub fn all_used(&self) -> u64 {
        self.used+self.memory 
    }

    pub fn remaining(&self) -> u64 {
        (self.limit - self.used) - self.memory
    }

    pub fn erase_cost(&mut self, returned: u64) {
        self.used -= returned;
    }

    pub fn record_refund(&mut self, refund: i64) {
        self.refunded += refund;
    }

    /// Record an explict cost.
    #[inline(always)]
    pub fn record_cost(&mut self, cost: u64) -> bool {
        let all_used_gas = self.used + self.memory + cost;
        if self.limit < all_used_gas {
            return false;
        }

        self.used += cost;
        true
    }

    #[inline(always)]
    pub fn record_cost_control(&mut self, cost: u64) -> Control {
        if !self.record_cost(cost) {
            return Control::Exit(ExitReason::Error(ExitError::OutOfGas));
        }
        Control::Continue
    }
}

impl Machine {
    pub fn new(contract: Contract, gas_limit: u64) -> Self {
        Self {
            program_counter: 0,
            return_range: Range::default(),
            memory: Memory::new(10000),
            stack: Stack::new(10000),
            status: Ok(()),
            return_data_buffer: Bytes::new(),
            contract,
            gas: Gas::new(gas_limit),
        }
    }
    pub fn contract(&self) -> &Contract {
        &self.contract
    }

    pub fn gas(&mut self) -> &Gas {
        &self.gas
    }

    /// used in gas_refund! macro
    pub fn gas_refund(&mut self, refund: i64) {
        self.gas.refunded += refund;
    }

    /// used in memory_resize! macro
    pub fn gas_memory(&mut self, gas_memory: u64) {
        self.gas.memory = max(self.gas.memory, gas_memory);
    }

    /// Reference of machine stack.
    pub fn stack(&self) -> &Stack {
        &self.stack
    }
    /// Mutable reference of machine stack.
    pub fn stack_mut(&mut self) -> &mut Stack {
        &mut self.stack
    }
    /// Reference of machine memory.
    pub fn memory(&self) -> &Memory {
        &self.memory
    }
    /// Mutable reference of machine memory.
    pub fn memory_mut(&mut self) -> &mut Memory {
        &mut self.memory
    }
    /// Return a reference of the program counter.
    pub fn program_counter(&self) -> usize {
        self.program_counter
    }

    /// loop steps until we are finished with execution
    pub fn run<H: ExtHandler, SPEC: Spec>(&mut self, handler: &mut H) -> ExitReason {
        loop {
            if let Err(reson) = self.step::<H, SPEC>(handler) {
                return reson;
            }
        }
    }

    #[inline]
    /// Step the machine, executing one opcode. It then returns.
    pub fn step<H: ExtHandler, SPEC: Spec>(&mut self, handler: &mut H) -> Result<(), ExitReason> {
        let program_counter = self.program_counter;

        // extract next opcode from code
        let opcode = self
            .contract
            .code
            .get(program_counter)
            .map(|&opcode| OpCode::try_from_u8(opcode))
            .flatten();
        // if there is no opcode in code or OpCode is invalid, return error.
        if opcode.is_none() {
            self.status = Err(ExitSucceed::Stopped.into());
            return Err(ExitSucceed::Stopped.into()); // TODO this not seems right, for invalid opcode
        }
        let opcode = opcode.unwrap();

        // call prevalidation to calcuate gas consumption for this opcode
        handler.trace_opcode(opcode, &self);

        // check machine status and return if not present
        self.status.as_ref().map_err(|reason| reason.clone())?;

        // evaluate opcode/execute instruction
        match eval::<H, SPEC>(self, opcode, program_counter, handler) {
            Control::Continue => {
                self.program_counter = program_counter + 1;
                Ok(())
            }
            Control::ContinueN(p) => {
                self.program_counter = program_counter + p;
                Ok(())
            }
            Control::Exit(e) => {
                self.status = Err(e.clone());
                Err(e)
            }
            Control::Jump(p) => {
                self.program_counter = p;
                Ok(())
            }
        }
    }

    /// Copy and get the return value of the machine, if any.
    pub fn return_value(&self) -> Bytes {
        if self.return_range.start > U256::from(usize::MAX) {
            let mut ret = Vec::new();
            ret.resize(
                (self.return_range.end - self.return_range.start).as_usize(),
                0,
            );
            Bytes::from(ret)
        } else if self.return_range.end > U256::from(usize::MAX) {
            let mut ret = self
                .memory
                .get(
                    self.return_range.start.as_usize(),
                    usize::MAX - self.return_range.start.as_usize(),
                )
                .to_vec();
            while ret.len() < (self.return_range.end - self.return_range.start).as_usize() {
                ret.push(0);
            }
            Bytes::from(ret)
        } else {
            self.memory.get(
                self.return_range.start.as_usize(),
                (self.return_range.end - self.return_range.start).as_usize(),
            )
        }
    }
}