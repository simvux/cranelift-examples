//! This example shows how to lower aggregate types such as structs.
//!
//! We'll go over how they can be constructed, optimized, and passed across the
//! function parameter/return boundry.
//!
//! The input we'll be working with is a AST-like `Type` type and a lookup table.
//!
//! Things to keep in mind for your own compiler:
//!
//! * Usually, things like field names and stringly identifiers would've already been desugared in
//! a previous IR before its time to lower into LLVM/Cranelift IR.
//!
//! * This example will *not* go over alignment. Which makes it inefficient and incompatible with ABI's.

use cranelift::{
    codegen::Context,
    prelude::{self as cl, FunctionBuilderContext, InstBuilder},
};
use cranelift_examples::skip_boilerplate;
use cranelift_module::{FuncId, Linkage, Module};

mod lower;
mod types;

use cranelift_object::ObjectModule;
use lower::FuncLower;
use types::{Resolver, Type};

#[derive(Clone, Debug)]
enum VirtualValue {
    // A singular value, will generally end up being passed around in registers.
    Scalar(cl::Value),

    // Implicitly putting some structs on the heap can prevent repeated memcpy in some cases
    // HeapStruct { ptr: cl::Value },

    // Our primary way of storing structs will be to create stackslots and write the fields at
    // offsets of the stackslot pointers.
    StackStruct {
        type_: &'static str,
        ptr: cl::Value,
    },

    // Instead of writing structs to stack pointers right away, we can try holding on to them in
    // registers for a bit in-case they're temporary or will be written to other struct pointers.
    UnstableStruct {
        type_: &'static str,
        fields: Vec<VirtualValue>,
    },
}

impl VirtualValue {
    fn unit() -> Self {
        VirtualValue::UnstableStruct {
            type_: "unit",
            fields: vec![],
        }
    }

    #[track_caller]
    fn as_scalar(&self) -> cl::Value {
        match self {
            VirtualValue::Scalar(value) => *value,
            _ => panic!("not an scalar value"),
        }
    }
}

fn main() {
    skip_boilerplate(b"struct-and-enum", |ctx, fctx, module, _args| {
        let mut types = types::Resolver::hardcoded(module.isa().pointer_bytes() as u32);

        let main_func_id = declare_main(module, &types);
        let move_right_func_id = declare_move_right(module, &types);

        types.function_names.insert(main_func_id, "main");
        types
            .function_names
            .insert(move_right_func_id, "move_right");

        define_main(module, &types, ctx, fctx, move_right_func_id, main_func_id);
        define_move_right(module, &types, ctx, fctx, move_right_func_id);
    });
}

// fn main();
fn declare_main(module: &mut ObjectModule, types: &Resolver) -> FuncId {
    let call_conv = module.isa().default_call_conv();
    let sig = types.create_signature(call_conv, "main");

    module
        .declare_function("main", Linkage::Export, &sig)
        .unwrap()
}

// fn move_right(p: Player, by: int) -> Player;
fn declare_move_right(module: &mut ObjectModule, types: &Resolver) -> FuncId {
    let call_conv = module.isa().default_call_conv();
    let sig = types.create_signature(call_conv, "move_right");

    module
        .declare_function("move_right", Linkage::Export, &sig)
        .unwrap()
}

// fn main() {
//   move_right(Player {
//      id: 5,
//      position: Point { x: 10, y: 20 },
//   }, 2);
// }
fn define_main(
    module: &mut ObjectModule,
    types: &Resolver,
    ctx: &mut Context,
    fctx: &mut FunctionBuilderContext,
    move_right_func_id: FuncId,
    id: FuncId,
) {
    ctx.func.signature = module
        .declarations()
        .get_function_decl(id)
        .signature
        .clone();
    let mut builder = cl::FunctionBuilder::new(&mut ctx.func, fctx);

    let mut lower = FuncLower::new(&types, &mut builder, module);
    let (entry, _vparams) = lower.create_entry_block(&[]);
    lower.fbuilder.switch_to_block(entry);

    let player = {
        let id = lower.int(5);

        let position = {
            let x = lower.int(10);
            let y = lower.int(20);

            lower.construct_struct("Point", &[("x", x), ("y", y)])
        };

        lower.construct_struct("Player", &[("id", id), ("position", position)])
    };

    let _move_right_call = {
        let two = lower.ins().iconst(cl::types::I64, 2);
        lower.call_func(move_right_func_id, vec![player, VirtualValue::Scalar(two)]);
    };

    // We don't want to return anything from main
    lower.return_(VirtualValue::unit());

    builder.finalize();
    dbg!(&ctx.func);

    module.define_function(id, ctx).unwrap();
    ctx.clear();
}

// fn move_right(p: Player, by: int) -> Player {
//    Player {
//      id: p.id,
//      position: Point {
//          x: p.position.x + by,
//          y: p.position.y,
//      }
//    }
// }
//
// // -- Although what we'll actually be lowering it into is something more like -- //
//
// fn move_right(ret: *Player, p: *Player, by: int) -> () {
//    *(ret+0) = *(p+0);
//    *(ret+8) = *(p+8) + by;
//    *(ret+16) = *(p+16);
// }
fn define_move_right(
    module: &mut ObjectModule,
    types: &Resolver,
    ctx: &mut Context,
    fctx: &mut FunctionBuilderContext,
    id: FuncId,
) {
    ctx.func.signature = module
        .declarations()
        .get_function_decl(id)
        .signature
        .clone();
    let mut builder = cl::FunctionBuilder::new(&mut ctx.func, fctx);

    let mut lower = FuncLower::new(&types, &mut builder, module);
    let (entry, vparams) = lower.create_entry_block(&[Type::Struct("Player"), Type::Int]);
    lower.fbuilder.switch_to_block(entry);

    let player = {
        let id = lower.destruct_field(&vparams[0], types.resolve_field("Player", "id"));

        let position = {
            let p_position =
                lower.destruct_field(&vparams[0], types.resolve_field("Player", "position"));

            let x = {
                let x = lower
                    .destruct_field(&p_position, types.resolve_field("Point", "x"))
                    .as_scalar();

                let by = vparams[1].as_scalar();
                VirtualValue::Scalar(lower.ins().iadd(x, by))
            };

            let y = lower.destruct_field(&p_position, types.resolve_field("Point", "y"));
            lower.construct_struct("Point", &[("x", x), ("y", y)])
        };

        lower.construct_struct("Player", &[("id", id), ("position", position)])
    };

    lower.return_(player);

    builder.finalize();
    dbg!(&ctx.func);

    module.define_function(id, ctx).unwrap();
    ctx.clear();
}
