use super::{VirtualValue, types};
use crate::types::Type;
use cranelift::codegen::ir;
use cranelift::frontend::FuncInstBuilder;
use cranelift::prelude::InstBuilder;
use cranelift::prelude::{self as cl, MemFlags};
use cranelift_module::{FuncId, Module};
use cranelift_object::ObjectModule;

/// The lowering of a single function to a Cranelift function
pub struct FuncLower<'a, 'f> {
    pub fbuilder: &'a mut cl::FunctionBuilder<'f>,
    pub module: &'a mut ObjectModule,
    types: &'a types::LookupTable,
}

impl<'a, 'f> FuncLower<'a, 'f> {
    pub fn new(
        types: &'a types::LookupTable,
        fbuilder: &'a mut cl::FunctionBuilder<'f>,
        module: &'a mut ObjectModule,
    ) -> Self {
        Self {
            fbuilder,
            module,
            types,
        }
    }

    pub fn ins(&mut self) -> FuncInstBuilder<'_, 'f> {
        self.fbuilder.ins()
    }

    // // In a real compiler, you'd most likely have something like this.
    // // Which would then match over the Expr and call the various helper methods we've defined here.
    //
    // pub fn expr(&mut self, expr: &ast::Expr) -> VirtualValue {...}

    /// Create the entry block with the appropriate Cranelift type signature
    ///
    /// Maps the Cranelift function parameters to our virtual values.
    pub fn create_entry_block(&mut self, params: &[Type]) -> (cl::Block, Vec<VirtualValue>) {
        let block = self.fbuilder.create_block();
        self.fbuilder.seal_block(block);

        // See `LookupTable::create_signature` for more information
        if self.fbuilder.func.signature.uses_struct_return_param() {
            let size_t = self.module.isa().pointer_type();
            self.fbuilder.append_block_param(block, size_t);
        }

        let vparams = params
            .iter()
            .map(|&p| self.type_to_block_params(block, true, p))
            .collect();

        (block, vparams)
    }

    // Turns a parameter from our source language into Cranelift block parameters.
    //
    // Since Cranelift parameters can only be primitive types, a single struct will either
    // become a single Cranelift pointer block parameter or multiple block parameters.
    fn type_to_block_params(&mut self, block: cl::Block, is_root: bool, p: Type) -> VirtualValue {
        self.type_to_virtual_value(
            &mut |this, clty| this.fbuilder.append_block_param(block, clty),
            is_root,
            p,
        )
    }

    // Maps our abstract Type to our abstract VirtualValue
    fn type_to_virtual_value<F>(&mut self, f: &mut F, is_root: bool, p: Type) -> VirtualValue
    where
        F: FnMut(&mut Self, cl::Type) -> cl::Value,
    {
        match p {
            Type::Unit => VirtualValue::unit(),
            Type::Int => {
                let v = f(self, cl::types::I32);
                VirtualValue::Scalar(v)
            }
            Type::Struct(type_) => {
                if is_root
                    && self.types.struct_passing_mode(type_) == types::StructPassingMode::ByPointer
                {
                    let size_t = self.module.isa().pointer_type();
                    let ptr = f(self, size_t);
                    VirtualValue::StackStruct { type_, ptr }
                } else {
                    let fields = self
                        .types
                        .fields_of_struct(type_)
                        .map(|(_, _, ty)| self.type_to_virtual_value(f, false, ty))
                        .collect();

                    VirtualValue::UnstableStruct { type_, fields }
                }
            }
        }
    }

    // Turns our virtual values into Cranelift parameters for the call instruction.
    //
    // Since Cranelift parameters can only be primitive types, a single struct will either
    // become a single Cranelift pointer value or multiple Cranelift values.
    fn virtual_value_to_func_params(&mut self, buf: &mut Vec<cl::Value>, v: VirtualValue) {
        match v {
            VirtualValue::Scalar(value) => buf.push(value),
            VirtualValue::StackStruct { type_, ptr: src } => {
                match self.types.struct_passing_mode(type_) {
                    types::StructPassingMode::ByScalars => {
                        self.deref_fields(buf, type_, src, 0);
                    }
                    types::StructPassingMode::ByPointer => buf.push(src),
                }
            }
            VirtualValue::UnstableStruct { type_, fields } => {
                match self.types.struct_passing_mode(type_) {
                    types::StructPassingMode::ByScalars => {
                        self.virtual_values_to_func_params(buf, fields)
                    }
                    types::StructPassingMode::ByPointer => {
                        let ptr = self.stack_alloc_struct(type_);
                        for (field, v) in fields.into_iter().enumerate() {
                            self.write_struct_field(type_, field, ptr, v);
                        }
                        buf.push(ptr);
                    }
                }
            }
        }
    }

    fn virtual_values_to_func_params(&mut self, buf: &mut Vec<cl::Value>, vs: Vec<VirtualValue>) {
        vs.into_iter()
            .for_each(|v| self.virtual_value_to_func_params(buf, v));
    }

    // Get the pointer parameter declared by the `LookupTable::create_signature` method
    //
    // This will for most targets be the first parameter.
    fn struct_return_pointer(&mut self) -> cl::Value {
        self.fbuilder
            .func
            .special_param(ir::ArgumentPurpose::StructReturn)
            .expect("current function does not return large struct")
    }

    pub fn call_func(&mut self, func: FuncId, params: Vec<VirtualValue>) -> VirtualValue {
        let mut call_params = vec![];

        let ret = self.types.return_type_of(func);

        // If the return type is too large to fit in return registers, we allocate space for it in
        // the current stack frame and pass a pointer as the first parameter for the child function to
        // write its return values to.
        let mut out_ptr_return = None;
        if let Type::Struct(name) = ret {
            if self.types.struct_passing_mode(name) == types::StructPassingMode::ByPointer {
                let ptr = self.stack_alloc_struct(name);
                call_params.push(ptr);
                out_ptr_return = Some(VirtualValue::StackStruct { type_: name, ptr });
            }
        }

        self.virtual_values_to_func_params(&mut call_params, params);

        let mut register_returns = {
            // In order to call a function, we need to first map a global FuncId into a local FuncRef
            // inside the current.
            let fref = self
                .module
                .declare_func_in_func(func, &mut self.fbuilder.func);

            let call = self.ins().call(fref, &call_params);

            self.fbuilder.inst_results(call).to_vec().into_iter()
        };

        // If the return values were handled through an out pointer, return that pointer
        // Otherwise; collect the returned scalar values into a VirtualValue to turn it back into our typed abstraction.
        out_ptr_return.unwrap_or_else(|| {
            self.type_to_virtual_value(&mut |_, _| register_returns.next().unwrap(), false, ret)
        })
    }

    pub fn int(&mut self, n: i64) -> VirtualValue {
        let v = self.ins().iconst(cl::types::I32, n);
        VirtualValue::Scalar(v)
    }

    pub fn construct_struct(
        &mut self,
        type_: &'static str,
        fields: &[(&str, VirtualValue)],
    ) -> VirtualValue {
        let fields = self
            .types
            .fields_of_struct(type_)
            .map(|(_, fname, _)| {
                fields
                    .iter()
                    .find_map(|(name, v)| (*fname == **name).then_some(v))
                    .cloned()
                    .expect("missing field in struct constructor")
            })
            .collect();

        VirtualValue::UnstableStruct { type_, fields }
    }

    pub fn destruct_field(&mut self, of: &VirtualValue, field: usize) -> VirtualValue {
        match of {
            VirtualValue::Scalar(_) => panic!("cannot destruct field from non-struct"),

            VirtualValue::StackStruct { type_, ptr } => {
                let offset = self.types.offset_of_field(type_, field);

                match self.types.type_of_field(type_, field) {
                    // Instead of actually dereferencing the inner struct here,
                    // we create another implicit stack pointer that's offset to where the inner struct starts.
                    //
                    // This makes dereferencing lazy.
                    Type::Struct(type_) => {
                        let nptr = self.ins().iadd_imm(*ptr, offset as i64);
                        VirtualValue::StackStruct { type_, ptr: nptr }
                    }
                    Type::Unit => VirtualValue::unit(),
                    Type::Int => {
                        let v = self
                            .ins()
                            .load(cl::types::I32, MemFlags::new(), *ptr, offset);
                        VirtualValue::Scalar(v)
                    }
                }
            }

            VirtualValue::UnstableStruct { fields, .. } => fields[field].clone(),
        }
    }

    /// Return a value, either by writing to the return struct out pointer or by returning values directly.
    pub fn return_(&mut self, vv: VirtualValue) {
        match vv {
            VirtualValue::Scalar(value) => {
                self.fbuilder.ins().return_(&[value]);
            }
            VirtualValue::StackStruct { type_, ptr: src } => {
                match self.types.struct_passing_mode(type_) {
                    // We have a stack pointer but want to return in return registers
                    types::StructPassingMode::ByScalars => {
                        let mut buf = vec![];
                        self.deref_fields(&mut buf, type_, src, 0);
                        self.ins().return_(&buf);
                    }
                    // We have a stack pointer and we want to return by writing to the out pointer
                    types::StructPassingMode::ByPointer => {
                        let dst = self.struct_return_pointer();
                        self.copy_struct_fields(type_, src, dst);
                        self.ins().return_(&[]);
                    }
                }
            }
            VirtualValue::UnstableStruct { type_, fields } => {
                match self.types.struct_passing_mode(type_) {
                    types::StructPassingMode::ByScalars => {
                        let fields = fields
                            .iter()
                            .map(VirtualValue::as_scalar)
                            .collect::<Vec<_>>();

                        self.fbuilder.ins().return_(&fields);
                    }
                    // We have an abstract struct and we want to write the fields to an out pointer
                    types::StructPassingMode::ByPointer => {
                        let dst = self.struct_return_pointer();

                        for (field, v) in fields.into_iter().enumerate() {
                            self.write_struct_field(type_, field, dst, v);
                        }

                        self.ins().return_(&[]);
                    }
                }
            }
        }
    }

    fn deref_fields(
        &mut self,
        buf: &mut Vec<cl::Value>,
        type_: &str,
        src: cl::Value,
        src_offset: i32,
    ) {
        for (field, _, _) in self.types.fields_of_struct(type_) {
            let offset = self.types.offset_of_field(type_, field) + src_offset;
            let fty = self.types.type_of_field(type_, field);
            match fty {
                Type::Unit => {}
                Type::Int => {
                    let v = self
                        .ins()
                        .load(cl::types::I32, MemFlags::new(), src, offset);

                    buf.push(v);
                }
                Type::Struct(type_) => {
                    self.deref_fields(buf, type_, src, offset);
                }
            }
        }
    }

    fn copy_struct_fields(&mut self, type_: &str, src: cl::Value, dst: cl::Value) {
        for (field, _, fty) in self.types.fields_of_struct(type_) {
            let offset = self.types.offset_of_field(type_, field);

            match fty {
                Type::Unit => {}
                Type::Int => {
                    let n = self
                        .ins()
                        .load(cl::types::I32, MemFlags::new(), src, offset);

                    self.ins().store(MemFlags::new(), n, dst, offset);
                }
                Type::Struct(type_) => {
                    let src = self.ins().iadd_imm(src, offset as i64);
                    let dst = self.ins().iadd_imm(dst, offset as i64);

                    self.copy_struct_fields(type_, src, dst);
                }
            }
        }
    }

    fn write_struct_field(&mut self, name: &str, field: usize, ptr: cl::Value, v: VirtualValue) {
        let offset = self.types.offset_of_field(name, field);

        match v {
            VirtualValue::Scalar(value) => {
                self.ins().store(MemFlags::new(), value, ptr, offset);
            }

            VirtualValue::UnstableStruct { type_, fields } => {
                for (field, v) in fields.into_iter().enumerate() {
                    // let offset = offset + self.types.offset_of_field(type_, field);
                    let nptr = self.ins().iadd_imm(ptr, offset as i64);
                    self.write_struct_field(type_, field, nptr, v);
                }
            }

            VirtualValue::StackStruct {
                type_: src_type,
                ptr: src_ptr,
            } => {
                let nptr = self.ins().iadd_imm(ptr, offset as i64);
                self.copy_struct_fields(src_type, src_ptr, nptr);
            }
        }
    }

    // Allocate the struct on the stack and return the stack pointer
    //
    // For this example we will be skipping caring about alignment, even though alignment is a
    // requirement for performance.
    pub(super) fn stack_alloc_struct(&mut self, name: &str) -> cl::Value {
        let size = self.types.size_of_struct(name);
        let slot = self.fbuilder.create_sized_stack_slot(cl::StackSlotData {
            kind: cl::StackSlotKind::ExplicitSlot,
            size,
            align_shift: 0,
        });

        let size_t = self.module.isa().pointer_type();
        self.ins().stack_addr(size_t, slot, 0)
    }
}
