//! This example shows some common ways to represent structs in memory.
//!
//! We will ensure all our structs are aligned. This substantially improves performance since it
//! lowers the amount of loads a CPU has to do, and is a hard requirement for a lot of ABI's.
//!
//! We will not be covering nested structs or circular structs here. But if you are, then keep in
//! mind that you will need to check for recursive data types, and either fail to compile or
//! automatically box the fields to make the structs finitely sized.
//!
//! The main function will construct two structs. One small and one large.
//! These structs will then be given as parameter to a function that returns a new struct
//! where each field has been incremented.

use cranelift::codegen::ir::ArgumentPurpose;
use cranelift::prelude::isa::CallConv;
use cranelift::prelude::{InstBuilder, types};
use cranelift::{codegen::ir::StackSlot, prelude as cl};
use cranelift_module::{FuncId, Linkage, Module};

use cranelift_examples::{declare_main, function_builder_from_declaration, skip_boilerplate};
use cranelift_object::ObjectModule;

fn main() {
    skip_boilerplate(b"struct-and-enum", |ctx, fctx, module, _args| {
        let size_t = module.isa().pointer_type();

        let small_struct_fields = &[types::I32, types::I32];
        let large_struct_fields = &[types::I32, types::I8, types::I32, types::I16];

        let main_func_id = declare_main(module);
        let inc_large_funcid = declare_increment_large(module, large_struct_fields);
        let inc_small_funcid = declare_increment_small(module, small_struct_fields);

        // fn main() {
        //   let large_struct = LargeStruct {...};
        //   let small_struct = SmallStruct {...};
        //
        //   let _ = inc_large_struct(large_struct);
        //   let _ = inc_small_struct(small_struct);
        //
        //   return;
        // }
        {
            let (mut fbuilder, _) =
                function_builder_from_declaration(module, &mut ctx.func, fctx, main_func_id);

            // let large_struct = LargeStruct {
            //   a: 1, // i32
            //   b: 2, // i8
            //   c: 3, // i32
            //   d: 4, // i16
            // };
            let large_struct: cl::Value = {
                // For larger structs, we reserve space on the stack and pass it around as a pointer
                //
                // Assigning a field will be storing to that pointer
                // Accessing a field will be loading from that pointer.
                let struct_stack_slot: StackSlot =
                    stack_alloc(&mut fbuilder, size_of_struct(large_struct_fields));

                // Here we use the `stack_` prefixed instructions to act upon the `cl::StackSlot` directly.
                // However; in a real compiler it might be easier to first get the pointer as a `cl::Value`
                // by using `FunctionBuilder::ins().stack_addr(...)` and then using `FunctionBuilder::ins().store(...)`

                for (i, n) in [1, 2, 3, 4].into_iter().enumerate() {
                    let offset = offset_of_field(i, large_struct_fields);
                    let value = fbuilder.ins().iconst(large_struct_fields[i], n);
                    fbuilder.ins().stack_store(value, struct_stack_slot, offset);
                }

                // Since our structs are aligned, padding was added.
                //
                // let large_struct = LargeStruct {
                //   a: 1,  // i32,
                //   b: 2,  // i8,
                //   _pad0: // i24
                //   c: 3   // i32
                //   d: 4   // i16
                //   _pad1: // i16
                // };

                // Convert the stack slot to a `cl::Value` pointer
                fbuilder.ins().stack_addr(size_t, struct_stack_slot, 0)
            };

            // let small_struct = SmallStruct {
            //   a: 1, // i32
            //   b: 2, // i32
            // };
            let small_struct: Vec<cl::Value> = {
                // For smaller structs, it's often unecesarry to introduce indirection.
                // Just passing around the fields as values can allow the struct to remain entirely in
                // registers.
                [1, 2]
                    .into_iter()
                    .enumerate()
                    .map(|(i, n)| fbuilder.ins().iconst(small_struct_fields[i], n))
                    .collect()
            };

            // let _ = inc_large_struct(large_struct);
            let _incremented_large_struct: cl::Value = {
                let fref = module.declare_func_in_func(inc_large_funcid, &mut fbuilder.func);

                let out_ptr = {
                    let out_stack_slot =
                        stack_alloc(&mut fbuilder, size_of_struct(large_struct_fields));

                    fbuilder.ins().stack_addr(size_t, out_stack_slot, 0)
                };

                fbuilder.ins().call(fref, &[large_struct, out_ptr]);

                out_ptr
            };

            // let _ = inc_small_struct(small_struct);
            let _incremented_small_struct: Vec<cl::Value> = {
                let fref = module.declare_func_in_func(inc_small_funcid, &mut fbuilder.func);

                let call = fbuilder.ins().call(fref, &small_struct);

                fbuilder.inst_results(call).to_vec()
            };

            fbuilder.ins().return_(&[]);
            fbuilder.finalize();

            println!("fn main:\n{}", &ctx.func);

            module.define_function(main_func_id, ctx).unwrap();
        }

        // fn inc_large_struct(large: LargeStruct) -> LargeStruct {
        //   return LargeStruct {
        //     a: large.a + 1,
        //     b: large.b + 1,
        //     c: large.c + 1,
        //     d: large.d + 1,
        //   };
        // }
        //
        // // -- Although the way we represent it in Cranelift looks like -- //
        //
        // fn inc_large_struct(large: &LargeStruct, out: &LargeStruct) {
        //   (*out+0) = *(large+0) + 1;
        //   (*out+4) = *(large+4) + 1;
        //   (*out+8) = *(large+8) + 1;
        //   (*out+12) = *(large+12) + 1;
        // }
        {
            let (mut fbuilder, entry) =
                function_builder_from_declaration(module, &mut ctx.func, fctx, inc_large_funcid);

            // By using `trusted`, we're asserting to Cranelift that the field is aligned and the
            // pointer is valid.
            let flags = cl::MemFlags::trusted();

            let param = fbuilder.block_params(entry)[0];
            let out_pointer = fbuilder.block_params(entry)[1];

            for (i, &ty) in large_struct_fields.iter().enumerate() {
                let offset = offset_of_field(i, large_struct_fields);

                // Access the field
                let v = fbuilder.ins().load(ty, flags, param, offset);
                // Increment it
                let v = fbuilder.ins().iadd_imm(v, 1);

                // Write it to the second struct pointer
                fbuilder.ins().store(flags, v, out_pointer, offset);
            }

            // We don't return any values as we're using an out pointer instead
            fbuilder.ins().return_(&[]);
            fbuilder.finalize();

            println!("fn inc_large_struct:\n{}", &ctx.func);

            module.define_function(inc_large_funcid, ctx).unwrap();
        }

        // fn inc_small_struct(small: SmallStruct) -> SmallStruct {
        //   return SmallStruct {
        //     a: small.a + 1,
        //     b: small.b + 1,
        //   };
        // }
        {
            let (mut fbuilder, entry) =
                function_builder_from_declaration(module, &mut ctx.func, fctx, inc_small_funcid);

            let a = {
                let small_a = fbuilder.block_params(entry)[0];
                fbuilder.ins().iadd_imm(small_a, 1)
            };

            let b = {
                let small_b = fbuilder.block_params(entry)[1];
                fbuilder.ins().iadd_imm(small_b, 1)
            };

            fbuilder.ins().return_(&[a, b]);
            fbuilder.finalize();

            println!("fn inc_small_struct:\n{}", &ctx.func);

            module.define_function(inc_small_funcid, ctx).unwrap();
        }
    });
}

fn declare_increment_large(module: &mut ObjectModule, large_struct_fields: &[cl::Type]) -> FuncId {
    let size_t = module.isa().pointer_type();
    let struct_size = size_of_struct(large_struct_fields);

    let sig = cl::Signature {
        params: vec![
            // Setting this argument purpose will generate memcpy'ing of the struct before
            // crossing the function boundry, so that the instance of the struct available in
            // the called function is unique.
            cl::AbiParam::special(size_t, ArgumentPurpose::StructArgument(struct_size)),
            // Setting this argument purpose will ensure that the pointer to write the
            // returned result into will be put in the appropriate register according to
            // the architecture's standards.
            cl::AbiParam::special(size_t, ArgumentPurpose::StructReturn),
        ],

        // We're not directly returning values, but instead use an out parameter.
        returns: vec![],

        call_conv: CallConv::Fast,
    };

    module
        .declare_function("inc_large_struct", Linkage::Local, &sig)
        .unwrap()
}

fn declare_increment_small(module: &mut ObjectModule, small_struct_fields: &[cl::Type]) -> FuncId {
    let sig = cl::Signature {
        // Since it's only two scalar values, it's more efficient to pass the fields
        // individually in registers.
        params: small_struct_fields
            .iter()
            .copied()
            .map(cl::AbiParam::new)
            .collect(),

        // Since it's only two scalar values, it'll fit in the return registers
        returns: small_struct_fields
            .iter()
            .copied()
            .map(cl::AbiParam::new)
            .collect(),

        call_conv: CallConv::Fast,
    };

    module
        .declare_function("inc_small_struct", Linkage::Local, &sig)
        .unwrap()
}

fn stack_alloc(fbuilder: &mut cl::FunctionBuilder<'_>, size: u32) -> StackSlot {
    fbuilder.create_sized_stack_slot(cl::StackSlotData::new(
        cl::StackSlotKind::ExplicitSlot,
        size,
        0,
    ))
}

fn size_of_struct(fields: &[cl::Type]) -> u32 {
    let mut size = 0;

    // Go through all fields and incement size by each fields size and padding
    for &field in fields {
        size += field.bytes() as u32;

        // Add padding to ensure the field is aligned
        let align = alignment_of_scalar_type(field);
        let padding = (align - size % align) % align;
        size += padding;
    }

    // Add padding to the end of the struct to make the struct itself aligned
    let self_align = alignment_of_struct(fields);
    let end_padding = (self_align - size % self_align) % self_align;
    size += end_padding;

    size
}

fn alignment_of_scalar_type(of: cl::Type) -> u32 {
    of.bytes()
}

fn alignment_of_struct(fields: &[cl::Type]) -> u32 {
    let mut alignment = 0;

    // Since we don't have nested structs, the allignment of a struct is simply its largest field.
    for &field in fields {
        let field_alignment = alignment_of_scalar_type(field);
        alignment = alignment.max(field_alignment);
    }

    alignment
}

fn offset_of_field(field: usize, fields: &[cl::Type]) -> i32 {
    let mut offset = 0;

    // Go through all fields prior to this one and increment offset by their size and padding
    for &prior in fields.iter().take(field) {
        offset += prior.bytes() as i32;

        // Add padding to ensure the field is aligned
        let align = alignment_of_scalar_type(prior) as i32;
        let padding = (align - offset % align) % align;
        offset += padding;
    }

    offset
}
