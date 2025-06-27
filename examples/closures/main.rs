//! This example shows how one can implement dynamic closures with captures.
//!
//! Let's imagine a scenario such as
//! ```
//! let f0 = |x| a + x + 1;
//! let f1 = |x| a + x + b;
//! let fs = [f0, f1];
//! ```
//! These two closures capture different values. However; They're put in the same array.
//! This is meant to be valid as the closures have the same type signature `int -> int`.
//!
//! We need a way to type-erase the captures and make all closures of the same type have the same size.
//!
//! A simple way to accomplish this is to make all closure a pair of a function pointer, and and
//! opaque capture pointer.
//!
//! This way; the captures can be dynamically dereferenced from the pointer and all closures will
//! be the exact same size.
//!
//! ```
//! let f0 = { data: &(a)   , func: |data, x| (*data).a + x + 1 };
//! let f1 = { data: &(a, b), func: |data, x| (*data).a + x + (*data).b };
//! let fs = [f0, f1];
//! ```
//!
//! To link against system libraries and produce a binary on Linux or MacOS, you can use `gcc` or `clang`
//!
//! `$ cargo run --example closures -- -o closures.o`
//! `$ clang closures.o -o closures`
//! `$ ./closures; echo $?`

use cranelift::prelude::isa::CallConv;
use cranelift::prelude::{self as cl, InstBuilder, Type};
use cranelift::prelude::{FunctionBuilder, MemFlags};
use cranelift_examples::{
    declare_main, function_builder_from_declaration, signature_from_decl, skip_boilerplate,
};
use cranelift_module::{FuncId, Linkage, Module};
use cranelift_object::ObjectModule;

fn main() {
    skip_boilerplate(b"closures", |ctx, fctx, module, _args| {
        let main_func_id = declare_main(module);
        let f0_funcid = declare_f0_real_function(module);
        let f1_funcid = declare_f1_real_function(module);

        // fn main() {
        //   let a = 1;
        //   let b = 2;
        //   let x = 3;
        //
        //   let f0 = |x| a + x + 1;
        //   let f1 = |x| a + x + b;
        //
        //   let t = f0(x);
        //   let u = f1(x);
        //
        //   return t + u;
        // }
        {
            let (mut fbuilder, _) =
                function_builder_from_declaration(module, &mut ctx.func, fctx, main_func_id);

            // let a = 1;
            // let b = 2;
            // let x = 3;
            let [a, b, x] = [1, 2, 3].map(|n| fbuilder.ins().iconst(cl::types::I32, n));

            // let f0 = |x| a + x + 1;
            // let f1 = |x| a + x + b;
            //
            // // -- Although the way we represent it in Cranelift looks like -- //
            //
            // let f0 = { data: &(a)   , func: |data, x| (*data).a + x + 1 };
            // let f1 = { data: &(a, b), func: |data, x| (*data).a + x + (*data).b };
            let f0 = construct_closure(module, &mut fbuilder, f0_funcid, &[a]);
            let f1 = construct_closure(module, &mut fbuilder, f1_funcid, &[a, b]);

            // let t = f0(x);
            // let u = f1(x);
            //
            // // -- Although the way we represent it in Cranelift looks like -- //
            //
            // let t = (f0.func)(f0.data, x);
            // let u = (f1.func)(f1.data, x)
            let t = f0.call(&mut fbuilder, &[x])[0];
            let u = f1.call(&mut fbuilder, &[x])[0];

            // return t + u;
            let sum = fbuilder.ins().iadd(t, u);
            fbuilder.ins().return_(&[sum]);

            fbuilder.finalize();

            println!("fn main:\n{}", &ctx.func);

            module.define_function(main_func_id, ctx).unwrap();
        }

        // fn f0(a: int, x: int) -> int {
        //   return a + x + 1;
        // }
        {
            let (mut fbuilder, block) =
                function_builder_from_declaration(module, &mut ctx.func, fctx, f0_funcid);

            let a = fbuilder.block_params(block)[0];
            let x = fbuilder.block_params(block)[1];

            let n = fbuilder.ins().iadd(a, x);
            let n = fbuilder.ins().iadd_imm(n, 1);

            fbuilder.ins().return_(&[n]);

            fbuilder.finalize();

            println!("fn f0:\n{}", &ctx.func);

            module.define_function(f0_funcid, ctx).unwrap();
        }

        // fn f1(a: int, b: int, x: int) -> int {
        //   return a + x + b;
        // }
        {
            let (mut fbuilder, block) =
                function_builder_from_declaration(module, &mut ctx.func, fctx, f1_funcid);

            let a = fbuilder.block_params(block)[0];
            let b = fbuilder.block_params(block)[1];
            let x = fbuilder.block_params(block)[2];

            let n = fbuilder.ins().iadd(a, x);
            let n = fbuilder.ins().iadd(n, b);

            fbuilder.ins().return_(&[n]);

            fbuilder.finalize();

            println!("fn f1:\n{}", &ctx.func);

            module.define_function(f1_funcid, ctx).unwrap();
        }
    });
}

// Declare the underlying function for the closure `f0`.
//
// All the captures are implicitly added as parameter.
//
// fn f0(a: int, x: int) -> int { a + x + 1 }
fn declare_f0_real_function(module: &mut ObjectModule) -> FuncId {
    // (a: int, x: int) -> int
    let sig = cl::Signature {
        call_conv: CallConv::Fast,
        params: vec![cl::AbiParam::new(cl::types::I32); 2],
        returns: vec![cl::AbiParam::new(cl::types::I32)],
    };

    module
        .declare_function("f0_real_function", Linkage::Local, &sig)
        .unwrap()
}

// Declare the underlying function for the closure `f0`.
//
// All the captures are implicitly added as parameter.
//
// fn f1(a: int, b: int, x: int) -> int { a + x + 1 }
fn declare_f1_real_function(module: &mut ObjectModule) -> FuncId {
    // (a: int, b: int, x: int) -> int
    let sig = cl::Signature {
        call_conv: CallConv::Fast,
        params: vec![cl::AbiParam::new(cl::types::I32); 3],
        returns: vec![cl::AbiParam::new(cl::types::I32)],
    };

    module
        .declare_function("f1_real_function", Linkage::Local, &sig)
        .unwrap()
}

struct Closure {
    data: cl::Value,
    func: cl::Value,
    sig: cl::Signature,
}

impl Closure {
    fn call<'a>(
        &self,
        fbuilder: &'a mut FunctionBuilder<'_>,
        params: &[cl::Value],
    ) -> &'a [cl::Value] {
        let mut real_params = vec![self.data];
        real_params.extend_from_slice(params);
        let sigref = fbuilder.import_signature(self.sig.clone());
        let call = fbuilder
            .ins()
            .call_indirect(sigref, self.func, &real_params);
        fbuilder.inst_results(call)
    }
}

// When invoking the closure, we can't know the types of the captures.
// However; here where we construct the closure we do know the types.
//
// To make this work we need to perform some form of type erasure, to make all closures with
// the same signatures behave the same regardless of captures.
//
// We do that by first boxing all the captures, and then create an intermediate function which
// dereferences the captures and forwards them to the 'real' function pointer.
fn construct_closure(
    module: &mut ObjectModule,
    fbuilder: &mut FunctionBuilder<'_>,
    closure_fn: FuncId,
    captures: &[cl::Value],
) -> Closure {
    let boxed_captures = stack_alloc_captures(module, fbuilder, captures);

    let (forwarding_func_ref, sig) = {
        let capture_types = captures
            .iter()
            .map(|&v| fbuilder.func.stencil.dfg.value_type(v))
            .collect::<Vec<_>>();

        let (func_id, sig) = create_forwarding_func(module, closure_fn, &capture_types);

        let fref = module.declare_func_in_func(func_id, &mut fbuilder.func);
        let size_t = module.isa().pointer_type();
        (fbuilder.ins().func_addr(size_t, fref), sig)
    };

    Closure {
        data: boxed_captures,
        func: forwarding_func_ref,
        sig,
    }
}

// If we have a closure with the user-facing signature `(int, int) -> int`
//
// Then the closure's actual signature will be `(*void, int, int) -> int`
// Where `*void` represents a pointer to the captures.
//
// We need to dereferences those captures and forward them to the real function defined where the
// closure is created (in this example `f0_real_function` and `f1_real_function`).
//
// We do so with what we here call the "forwarding function".
//
// So for the `f1` we'd define.
//
// ```
// fn closure_forward_f1_real_function(captures: *void, x: int) -> int {
//   let a = *(captures + 0);
//   let b = *(captures + 4);
//   return f1_real_function(a, b, x);
// }
// ```
//
// And then the actual values we will pass around in memory would be.
// ```
// let closure = { data: alloc([1, 2]), func: closure_forward_f1_real_function };
// ```
//
// So that it may be called as
//
// ```
// closure.func(closure.data, 3)
// ```
fn create_forwarding_func(
    module: &mut ObjectModule,
    f: FuncId,
    captys: &[Type],
) -> (FuncId, cl::Signature) {
    // In a real compiler, this symbol needs to be generated in a way that's garenteed to be
    // unique. You could for example use source code spans, capture type information, or a global counter.
    let symbol = format!("closure_forward_{f}");

    // Define the signature of the forwarding function to be that of the closure signature but
    // with the opaque captures pointer added as the first parameter.
    let sig = {
        let mut sig = cl::Signature::new(CallConv::Fast);

        // The implicit parameters from the capture will be replaced by an opaque pointer instead.
        let voidptr = cl::AbiParam::new(module.isa().pointer_type());
        sig.params.insert(0, voidptr);

        let real_func_sig = signature_from_decl(module, f);
        for &p in real_func_sig.params.iter().skip(captys.len()) {
            sig.params.push(p);
        }
        sig.returns = real_func_sig.returns.clone();

        sig
    };

    // Declare the closure forwarding function
    let func_id = module
        .declare_function(&symbol, Linkage::Local, &sig)
        .unwrap();

    // Define the contents of the closure forwarding function
    {
        let mut ctx = cl::codegen::Context::new();
        let mut fctx = cl::FunctionBuilderContext::new();

        let mut closure = cl::FunctionBuilder::new(&mut ctx.func, &mut fctx);
        closure.func.signature = sig.clone();

        let block = closure.create_block();
        closure.append_block_params_for_function_params(block);
        closure.switch_to_block(block);

        let mut real_call_params =
            Vec::with_capacity(captys.len() + closure.func.signature.params.len() - 1);

        // Dereference the captures and add them as implicit parameters
        let mut offset = 0;
        for &ty in captys {
            let ptr = closure.block_params(block)[0];
            let v = closure.ins().load(ty, MemFlags::new(), ptr, offset);
            real_call_params.push(v);
            offset += ty.bytes() as i32;
        }

        // Add all other parameters from the forwarding function
        for &v in &closure.block_params(block)[1..] {
            real_call_params.push(v);
        }

        let f_ref = module.declare_func_in_func(f, &mut closure.func);
        let call = closure.ins().call(f_ref, &real_call_params);
        let returned = closure.inst_results(call).to_vec();
        closure.ins().return_(&returned);

        module.define_function(func_id, &mut ctx).unwrap();
    };

    (func_id, sig)
}

fn stack_alloc_captures(
    module: &ObjectModule,
    fbuilder: &mut FunctionBuilder<'_>,
    captures: &[cl::Value],
) -> cl::Value {
    let size_t = module.isa().pointer_type();

    // Unlike the `struct-layouts` example, we will not be caring about alignment or padding here.
    //
    // So the size of the stack allocation will just be the sum of the fields we're allocating.
    let size = captures
        .iter()
        .map(|&v| type_of_value(fbuilder, v).bytes())
        .sum();

    // Create the stack slot for the captures
    let slot = fbuilder.create_sized_stack_slot(cl::StackSlotData::new(
        cl::StackSlotKind::ExplicitSlot,
        size,
        0,
    ));

    // Write our captures to the stack allocation
    let mut offset = 0;
    for &v in captures {
        fbuilder.ins().stack_store(v, slot, offset);
        offset += type_of_value(fbuilder, v).bytes() as i32;
    }

    // Return the pointer
    fbuilder.ins().stack_addr(size_t, slot, 0)
}

fn type_of_value(fbuilder: &FunctionBuilder<'_>, v: cl::Value) -> Type {
    fbuilder.func.stencil.dfg.value_type(v)
}
