//! This example shows some common ways to represent tagged unions (`enum`) in memory.
//!
//! For performant results, you ideally want to lower differently sized enums/variants in different
//! ways.
//!
//! When the enum doesn't have any variants with parameters, and is just a tag.
//! Then it's simply named constants for an integer.
//!
//! For variants with payloads then a straight forward way to implement them is to have a tag+pointer.
//! We will then select upon the tag and cast the pointer into different data depending on the case.
//!
//! If the payload of a variant is small enough, then instead of being stored indirectly through a
//! pointer, the pointer can be treated as an inlined integer scalar which is then reduced to the right
//! size depending on the case.
//!
//! In this example all tagged union types will have the size `TAG_TYPE.bytes() + size_t`
//!
//! To link against system libraries and produce a binary on Linux or MacOS, you can use `gcc` or `clang`
//!
//! `$ cargo run --example tagged-union-layouts -- -o tagged-union-layouts.o`
//! `$ clang tagged-union-layouts.o -o tagged-union-layouts`
//! `$ ./tagged-union-layouts; echo $?`

use cranelift::codegen::ir::BlockCall;
use cranelift::prelude as cl;
use cranelift::prelude::{FunctionBuilder, InstBuilder, JumpTableData, types};
use cranelift_examples::{declare_main, function_builder_from_declaration, skip_boilerplate};
use cranelift_module::Module;
use cranelift_object::ObjectModule;
use std::cmp::Ordering;

const TAG_TYPE: cl::Type = cl::types::I32;

// enum Packet {
//   Pending,
//   Data(I32, I32, I32),
//   Failed(i32),
// }
const TAG_PACKET_PENDING: i64 = 0;
const TAG_PACKET_DATA: i64 = 1;
const TAG_PACKET_FAILED: i64 = 2;

fn main() {
    skip_boilerplate(b"tagged-union-layouts", |ctx, fctx, module, _args| {
        let size_t = module.isa().pointer_type();

        let main_func_id = declare_main(module);

        // fn main() -> i32 {
        //   let packet_data = Packet::Data(1, 2, 3);
        //   let packet_pending = Packet::Pending;
        //   let packet_failed = Packet::Failed(100);
        //
        //   let matched = packet_data;
        //
        //   match matched {
        //     Packet::Pending => return 10,
        //     Packet::Data(x, y, z) => return x + y + z,
        //     Packet::Failed(code) => return code,
        //   }
        // }
        {
            let (mut fbuilder, _) =
                function_builder_from_declaration(module, &mut ctx.func, fctx, main_func_id);

            // let packet_data = Packet::Data(10, 20, 30)
            let packet_data = {
                let one = fbuilder.ins().iconst(cl::types::I32, 10);
                let two = fbuilder.ins().iconst(cl::types::I32, 20);
                let three = fbuilder.ins().iconst(cl::types::I32, 30);

                construct_tagged_union(module, &mut fbuilder, TAG_PACKET_DATA, &[one, two, three])
            };

            // let packet_pending = Packet::Pending
            //
            // Even though this variant doesn't have a payload, all values of type `Packet`
            // still needs to have the same size. Therefore we still create a zeroed inlined payload.
            let _packet_pending =
                construct_tagged_union(module, &mut fbuilder, TAG_PACKET_PENDING, &[]);

            // let packet_failed = Packet::Failed(100)
            //
            // Since the variant parameter is small enough, it does not need a stack pointer.
            let _packet_failed = {
                let hundred = fbuilder.ins().iconst(cl::types::I32, 100);
                construct_tagged_union(module, &mut fbuilder, TAG_PACKET_FAILED, &[hundred])
            };

            // match matched {
            //   Packet::Pending => return 10,
            //   Packet::Data(x, y, z) => return x + y + z,
            //   Packet::Failed(code) => return code,
            // }
            {
                // Which of the constructed variants we're matching against
                let (tag, payload) = packet_data;

                // Declare all the blocks for the jump table branches
                let branches = [TAG_PACKET_PENDING, TAG_PACKET_DATA, TAG_PACKET_FAILED].map(|_| {
                    let block = fbuilder.create_block();
                    BlockCall::new(block, &[], &mut fbuilder.func.dfg.value_lists)
                });

                // Declare the block for the default branch
                let trap = {
                    let block = fbuilder.create_block();
                    BlockCall::new(block, &[], &mut fbuilder.func.dfg.value_lists)
                };

                // Create the table
                let table = {
                    let table_data = JumpTableData::new(trap, &branches);
                    fbuilder.func.create_jump_table(table_data)
                };

                // Set main's block terminator to the jump table
                fbuilder.ins().br_table(tag, table);

                // Packet::Pending => return 10,
                {
                    switch_to_branch_block(&mut fbuilder, branches[TAG_PACKET_PENDING as usize]);

                    let ten = fbuilder.ins().iconst(types::I32, 10);

                    fbuilder.ins().return_(&[ten]);
                }

                // Packet::Data(x, y, z) => return x + y + z,
                {
                    switch_to_branch_block(&mut fbuilder, branches[TAG_PACKET_DATA as usize]);

                    let params = [cl::types::I32, cl::types::I32, cl::types::I32];
                    let [x, y, z] = read_payload(size_t, &mut fbuilder, payload, params);

                    let sum = fbuilder.ins().iadd(x, y);
                    let sum = fbuilder.ins().iadd(sum, z);

                    fbuilder.ins().return_(&[sum]);
                }

                // Packet::Failed(code) => return code,
                {
                    switch_to_branch_block(&mut fbuilder, branches[TAG_PACKET_FAILED as usize]);

                    let [code] = read_payload(size_t, &mut fbuilder, payload, [cl::types::I32]);

                    fbuilder.ins().return_(&[code]);
                }

                // Trap the default block
                //
                // _ => unreachable!(),
                {
                    switch_to_branch_block(&mut fbuilder, trap);

                    const TRAP_UNREACHABLE: u8 = 100;

                    fbuilder
                        .ins()
                        .trap(cl::TrapCode::user(TRAP_UNREACHABLE).unwrap());
                }
            }

            fbuilder.finalize();

            println!("fn main:\n{}", &ctx.func);

            module.define_function(main_func_id, ctx).unwrap();
        }
    });
}

fn switch_to_branch_block(fbuilder: &mut FunctionBuilder<'_>, call: BlockCall) {
    let block = call.block(&fbuilder.func.dfg.value_lists);
    fbuilder.seal_block(block);
    fbuilder.switch_to_block(block);
}

// Convert the payload to the requested type.
//
// For larger payloads the `size_t` value will be treated as a pointer for us to read the
// variant parameters from.
//
// For smaller payloads, the `size_t` will be casted to the parameter.
fn read_payload<const N: usize>(
    size_t: cl::Type,
    fbuilder: &mut FunctionBuilder<'_>,
    payload: cl::Value,
    param_types: [cl::Type; N],
) -> [cl::Value; N] {
    match payload_kind(size_t, &param_types) {
        // Reduce the size of the payload to the inlined data size
        PayloadKind::InlineCasted(target) => {
            param_types.map(|_| fbuilder.ins().ireduce(target, payload))
        }

        // Use the payload as-is
        PayloadKind::Inline => param_types.map(|_| payload),

        // Use zero as the payload so that this payload-less variant still has the same size
        PayloadKind::Zero => param_types.map(|_| fbuilder.ins().iconst(size_t, 0)),

        // Dereference the fields from the payload stack pointer
        PayloadKind::StackPointer => {
            let mut offset = 0;
            param_types.map(|ty| {
                let v = fbuilder
                    .ins()
                    .load(ty, cl::MemFlags::new(), payload, offset);
                offset += ty.bytes() as i32;
                v
            })
        }
    }
}

fn construct_tagged_union(
    module: &ObjectModule,
    fbuilder: &mut FunctionBuilder<'_>,
    tag: i64,
    params: &[cl::Value],
) -> (cl::Value, cl::Value) {
    let size_t = module.isa().pointer_type();

    let param_types = params
        .iter()
        .map(|param| type_of_value(fbuilder, *param))
        .collect::<Vec<_>>();

    let payload = match payload_kind(size_t, &param_types) {
        PayloadKind::InlineCasted(_) => fbuilder.ins().sextend(size_t, params[0]),
        PayloadKind::Inline => params[0],
        PayloadKind::Zero => fbuilder.ins().iconst(size_t, 0),
        PayloadKind::StackPointer => stack_alloc_payload(module, fbuilder, params),
    };

    let tag = fbuilder.ins().iconst(TAG_TYPE, tag);

    (tag, payload)
}

enum PayloadKind {
    InlineCasted(cl::Type),
    Inline,
    Zero,
    StackPointer,
}

fn payload_kind(size_t: cl::Type, params: &[cl::Type]) -> PayloadKind {
    match params {
        // We want to inline the payload if it fits in the bytes of size_t
        [param] => {
            match param.bytes().cmp(&size_t.bytes()) {
                // Should be cast to size_t
                Ordering::Less => PayloadKind::InlineCasted(*param),
                // The scalar will already have the same memory layout as a payload
                Ordering::Equal => PayloadKind::Inline,
                // It doesn't fit in the bytes of size_t, so the payload will be stack allocated
                Ordering::Greater => PayloadKind::StackPointer,
            }
        }

        // It still needs to be the same size of other enums of the same type, so we generate a
        // zeroed payload.
        [] => PayloadKind::Zero,

        // Stack allocate larger payloads to store them behind a pointer.
        //
        // One possible optimization is to still inline the payload if it's multiple scalars that
        // fit within size_t by using `iconcat` and `isplit`.
        _ => PayloadKind::StackPointer,
    }
}

// Larger enum variants will store their data behind a pointer.
fn stack_alloc_payload(
    module: &ObjectModule,
    fbuilder: &mut FunctionBuilder<'_>,
    params: &[cl::Value],
) -> cl::Value {
    let size_t = module.isa().pointer_type();

    // Unlike the `struct-layouts` example, we will not be caring about alignment or padding here.
    //
    // So the size of the stack allocation will just be the sum of the fields we're allocating.
    let size = params
        .iter()
        .map(|&v| type_of_value(fbuilder, v).bytes())
        .sum();

    // Create the stack slot for the payload data
    let slot = fbuilder.create_sized_stack_slot(cl::StackSlotData::new(
        cl::StackSlotKind::ExplicitSlot,
        size,
        0,
    ));

    // Write our fields to the stack allocation
    let mut offset = 0;
    for &v in params {
        fbuilder.ins().stack_store(v, slot, offset);
        offset += type_of_value(fbuilder, v).bytes() as i32;
    }

    // Return the pointer
    fbuilder.ins().stack_addr(size_t, slot, 0)
}

fn type_of_value(fbuilder: &FunctionBuilder<'_>, v: cl::Value) -> cl::Type {
    fbuilder.func.stencil.dfg.value_type(v)
}
