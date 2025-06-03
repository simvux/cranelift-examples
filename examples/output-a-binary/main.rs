//! This example shows how to code-generate a function and emit it into an object file.
//!
//! How an object file is later turned into a runnable executable will depend on the operating
//! system you're on.
//!
//! You could produce a raw executable that isn't linked to any system libraries using `ld object.o`
//! and then declare an entrypoint function with the symbol the operating system expects ("_start" on Linux).
//!
//! In our examples we'll be linking to libc and declare "main" which is invoked by libc.
//! When linking against libc, the main function can return an exit code.
//!
//! To link against system libraries and produce a binary on Linux or MacOS, you can use `gcc` or `clang`
//!
//! `$ cargo run --example output-a-binary`
//! `$ clang output-a-binary.o -o output-a-binary`
//! `$ ./output-a-binary; echo $?`

use cranelift::prelude::*;
use cranelift_module::{Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::{fs::File, io::Write};

// The platform we're targetting.
//
// These constants may need to be changed if you're on MacOS/Windows.
const TARGET_TRIPLE: &str = "x86_64-unknown-linux";
const ENTRYPOINT_FUNCTION_SYMBOL: &str = "main";

fn main_signature(isa: &dyn isa::TargetIsa) -> Signature {
    // The `CallConv` defines how primitives in parameters and return values are handled.
    // Mainly which registers are used and when stack spills are used.
    //
    // In general it's best to use `CallConv::Fast`.
    //
    // However; since the function we define is invoked from our targetted OS, we need to use
    // the calling convention the OS expects.
    let call_conv = isa.default_call_conv();

    Signature {
        call_conv,
        params: vec![],
        // Since we're linking to libc, we can return the exit code from main.
        returns: vec![AbiParam::new(types::I32)],
    }
}

fn main() {
    // The ISA contains information about our intended target and acts as the settings for cranelift.
    let isa = {
        let mut builder = settings::builder();

        // disable optimizations so dissassembly will more directly correlated to our Cranelift usage
        builder.set("opt_level", "none").unwrap();

        let flags = settings::Flags::new(builder);

        isa::lookup_by_name(TARGET_TRIPLE)
            .unwrap()
            .finish(flags)
            .unwrap()
    };

    // Cranelift has the concept of a Module which ties declarations together.
    //
    // Module is actually a trait, and which implementation of this trait you use will depend on
    // what sort of environment you're generating code into.
    //
    // Our objective is to generate an ahead-of-time compiled binary.
    // So; we use the `cranelift-object` crate which exposes `ObjectModule` as a Module implementation.
    //
    // Object refers to object files (`.o` on unix-like systems and `.obj` on Windows).
    // These files contain unlinked machine code, and we can then use a 'linker' to merge them into our final executable.
    let mut module = {
        let translation_unit_name = b"output_a_binary";
        let libcall_names = cranelift_module::default_libcall_names();
        let builder =
            ObjectBuilder::new(isa.clone(), translation_unit_name, libcall_names).unwrap();
        ObjectModule::new(builder)
    };

    // First we declare our functions.
    // Adding which functions exist in the module and granting them their signatures.
    //
    // In this example there's only one function, the programs entrypoint.
    let main_declaration_func_id = {
        let sig = main_signature(&*isa);

        // Add this function to our Module.
        module
            .declare_function(ENTRYPOINT_FUNCTION_SYMBOL, Linkage::Export, &sig)
            .unwrap()
    };

    // Define the contents of our functions
    {
        // These contains the context needed for genering code for a function.
        //
        // It's a lot more efficient to construct them once, and then re-use them for all functions.
        let mut ctx = codegen::Context::new();
        let mut fctx = FunctionBuilderContext::new();

        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut fctx);
        builder.func.signature = main_signature(&*isa);

        // Create the functions entry block.
        let block0 = builder.create_block();
        builder.switch_to_block(block0);

        // When we know that there are no more other blocks which can jump to this block, we want to seal
        // it. This improves the quality of code generation.
        builder.seal_block(block0);

        let one = builder.ins().iconst(types::I32, 1);
        let two = builder.ins().iadd(one, one);

        // Use the result of the addition as an exit code
        builder.ins().return_(&[two]);

        if let Err(err) = codegen::verify_function(&builder.func, isa.as_ref()) {
            panic!("verifier error: {err}");
        }

        builder.finalize();

        println!("fn {ENTRYPOINT_FUNCTION_SYMBOL}:\n{}", &ctx.func);

        module
            .define_function(main_declaration_func_id, &mut ctx)
            .unwrap();

        ctx.clear();
    }

    // Finalize the module to generate our `Product`.
    //
    // If we have additional information such as unwind information or DWARF debug information,
    // they can be added to `Product`. For this example we skip such optional additions.
    let product = module.finish();

    // Generate the object file.
    {
        let bytes = product.emit().unwrap();

        let fname = "output-a-binary.o";
        let mut f = File::create(fname).unwrap();
        f.write_all(&bytes).unwrap();

        println!(" wrote output to {fname}");
    }
}
