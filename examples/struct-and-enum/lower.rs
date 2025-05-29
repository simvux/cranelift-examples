use super::{VirtualValue, types};
use crate::types::Type;
use cranelift::codegen::ir;
use cranelift::frontend::FuncInstBuilder;
use cranelift::prelude::InstBuilder;
use cranelift::prelude::{self as cl, MemFlags};
use cranelift_module::{FuncId, Module};
use cranelift_object::ObjectModule;

pub struct Lower<'a, 'f> {
    pub fbuilder: &'a mut cl::FunctionBuilder<'f>,
    pub module: &'a mut ObjectModule,
    types: &'a types::TypeResolver,
}

impl<'a, 'f> Lower<'a, 'f> {
    pub fn new(
        types: &'a types::TypeResolver,
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

    pub fn create_entry_block(&mut self, params: &[Type]) -> (cl::Block, Vec<VirtualValue>) {
        let block = self.fbuilder.create_block();
        let vparams = params
            .iter()
            .map(|&p| self.param_type_to_vv(block, true, p))
            .collect();
        (block, vparams)
    }

    // Turns a parameter from our source language into Cranelift block parameters.
    //
    // Since Cranelift parameters can only be more primitive types, a single struct will either
    // become a single Cranelift pointer value or multiple block parameters.
    fn param_type_to_vv(&mut self, block: cl::Block, is_root: bool, p: Type) -> VirtualValue {
        match p {
            Type::Int => {
                let v = self.fbuilder.append_block_param(block, cl::types::I64);
                VirtualValue::Scalar(v)
            }
            Type::Struct(type_) => {
                if is_root
                    && self.types.struct_passing_mode(type_) == types::StructPassingMode::ByPointer
                {
                    let size_t = self.module.isa().pointer_type();
                    let ptr = self.fbuilder.append_block_param(block, size_t);

                    VirtualValue::StackStruct { type_, ptr }
                } else {
                    let fields = self
                        .types
                        .fields_of_struct(type_)
                        .map(|(_, _, ty)| self.param_type_to_vv(block, false, ty))
                        .collect();

                    VirtualValue::UnstableStruct { type_, fields }
                }
            }
        }
    }

    // Turns our virtual values into Cranelift parameters for the call instructions.
    //
    // Since Cranelift parameters can only be more primitive types, a single struct will either
    // become a single Cranelift pointer value or multiple Cranelift values.
    fn vv_to_func_params(&mut self, buf: &mut Vec<cl::Value>, v: VirtualValue) {
        match v {
            VirtualValue::Scalar(value) => buf.push(value),
            VirtualValue::StackStruct { type_, ptr: src } => {
                match self.types.struct_passing_mode(type_) {
                    types::StructPassingMode::ByScalars => {
                        todo!("dereference the fields");
                    }
                    types::StructPassingMode::ByPointer => buf.push(src),
                }
            }
            VirtualValue::UnstableStruct { type_, fields } => {
                match self.types.struct_passing_mode(type_) {
                    types::StructPassingMode::ByScalars => {
                        fields
                            .into_iter()
                            .for_each(|v| self.vv_to_func_params(buf, v));
                    }
                    types::StructPassingMode::ByPointer => {
                        todo!("ok we do need an is_root marker for this. ");
                        // or we can just go into a different function
                    }
                }
            }
        }
    }

    fn struct_return_pointer(&mut self) -> cl::Value {
        self.fbuilder
            .func
            .special_param(ir::ArgumentPurpose::StructReturn)
            .expect("current function does not return large struct")
    }

    // // In a real compiler, you'd most likely have something like this.
    // // Which would then match over the Expr and call the various helper methods we've defined here.
    // pub fn expr(&mut self, expr: &ast::Expr) -> VirtualValue {...}

    pub fn call(&mut self, func: FuncId, params: Vec<VirtualValue>) -> VirtualValue {
        let mut buf = vec![];
        for p in params {
            self.vv_to_func_params(&mut buf, p);
        }

        todo!("we need to know whether we should give it a function return pointer or not");
        // And for that...... we need a lookup table for the func signatures
    }

    pub fn int(&mut self, n: i64) -> VirtualValue {
        let v = self.ins().iconst(cl::types::I64, n);
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

            // Instead of actually dereferencing it here, we create another implicit stack
            // pointer that's offset to where the inner struct starts.
            //
            // This makes dereferencing lazy.
            VirtualValue::StackStruct { type_, ptr } => {
                todo!();
            }

            VirtualValue::UnstableStruct { type_, fields } => {
                todo!();
            }
        }
    }

    pub fn return_(&mut self, vv: VirtualValue) {
        match vv {
            VirtualValue::Scalar(value) => {
                self.fbuilder.ins().return_(&[value]);
            }
            VirtualValue::StackStruct { type_, ptr: src } => {
                match self.types.struct_passing_mode(type_) {
                    types::StructPassingMode::ByScalars => {
                        let mut buf = vec![];
                        self.deref_fields(&mut buf, type_, src, 0);
                        self.ins().return_(&buf);
                    }
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
        for (field, _, fty) in self.types.fields_of_struct(type_) {
            let offset = self.types.offset_of_field(type_, field) + src_offset;
            match fty {
                Type::Int => {
                    self.ins()
                        .load(cl::types::I64, MemFlags::new(), src, offset);
                }
                Type::Struct(type_) => self.deref_fields(buf, type_, src, offset),
            }
        }
    }

    fn copy_struct_fields(&mut self, type_: &str, src: cl::Value, dst: cl::Value) {
        for (field, _, fty) in self.types.fields_of_struct(type_) {
            let offset = self.types.offset_of_field(type_, field);

            match fty {
                Type::Int => {
                    let n = self
                        .ins()
                        .load(cl::types::I64, MemFlags::new(), src, offset);

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
        let flags = MemFlags::new();

        match v {
            VirtualValue::Scalar(value) => {
                self.ins().store(flags, value, ptr, offset);
            }

            VirtualValue::UnstableStruct { type_, fields } => {
                todo!();
            }

            VirtualValue::StackStruct {
                type_: src_type,
                ptr: src_ptr,
            } => {
                let src_size = self.types.size_of_struct(name);
                let ptr_type = self.module.isa().pointer_type();
                let src_size = self.ins().iconst(ptr_type, src_size as i64);

                self.fbuilder
                    .call_memcpy(self.module.target_config(), ptr, src_ptr, src_size);
                todo!();
            }
        }
    }

    // Allocate the struct on the stack and return the stack pointer
    //
    // For this example we will be skipping caring about alignment, even though alignment is a
    // requirement for performance.
    fn stack_alloc_struct(&mut self, name: &str) -> cl::Value {
        let size = self.types.size_of_struct(name);
        let slot = self.fbuilder.create_sized_stack_slot(cl::StackSlotData {
            kind: cl::StackSlotKind::ExplicitSlot,
            size,
            align_shift: 0,
        });
        self.ins().stack_load(cl::types::I64, slot, 0)
    }
}
