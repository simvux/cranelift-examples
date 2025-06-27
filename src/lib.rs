use clap::{arg, command};
use cranelift::{
    codegen::ir::Function,
    prelude::{self as cl, Configurable, FunctionBuilder},
};
use cranelift_module::{FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::{fs::File, io::Write};

pub fn parse_arguments() -> clap::ArgMatches {
    command!()
        .arg(arg!(-t --"target-triple" <TRIPLE> "Target triple arch-vendor-platform"))
        .arg(arg!(-o --"output" <FILE> "Path for output object file"))
        .get_matches()
}

/// Performs initialization and finalization of cranelift similarly to the instructions provided in [output-a-binary](examples/output-a-binary/main.rs)
pub fn skip_boilerplate(
    unit_name: &[u8],
    f: impl FnOnce(
        &mut cl::codegen::Context,
        &mut cl::FunctionBuilderContext,
        &mut ObjectModule,
        clap::ArgMatches,
    ),
) {
    let args = parse_arguments();

    let isa = {
        let mut builder = cl::settings::builder();

        builder.set("opt_level", "none").unwrap();
        builder.enable("is_pic").unwrap();

        let flags = cl::settings::Flags::new(builder);

        let triple = args
            .get_one::<&str>("target-triple")
            .unwrap_or(&"x86_64-unknown-linux");

        cl::isa::lookup_by_name(triple)
            .unwrap()
            .finish(flags)
            .unwrap()
    };

    let mut module = {
        let libcall_names = cranelift_module::default_libcall_names();
        let builder = ObjectBuilder::new(isa.clone(), unit_name, libcall_names).unwrap();
        ObjectModule::new(builder)
    };

    let path: Option<String> = args.get_one("output").cloned();

    let mut ctx = cl::codegen::Context::new();
    let mut fctx = cl::FunctionBuilderContext::new();

    f(&mut ctx, &mut fctx, &mut module, args);

    let product = module.finish();

    match path {
        Some(path) => {
            let bytes = product.emit().unwrap();

            let mut f = File::create(&path).unwrap();
            f.write_all(&bytes).unwrap();

            println!(" wrote output to {} ", path);
        }
        None => {
            println!(" no `-o` path specified ");
        }
    }
}

pub fn function_builder_from_declaration<'a>(
    module: &mut ObjectModule,
    func: &'a mut Function,
    fctx: &'a mut cl::FunctionBuilderContext,
    func_id: FuncId,
) -> (FunctionBuilder<'a>, cl::Block) {
    func.clear();
    let mut fbuilder = cl::FunctionBuilder::new(func, fctx);
    fbuilder.func.signature = signature_from_decl(module, func_id);
    let entry = create_entry_block(&mut fbuilder);
    fbuilder.switch_to_block(entry);
    (fbuilder, entry)
}

pub fn signature_from_decl(module: &ObjectModule, func: FuncId) -> cl::Signature {
    module
        .declarations()
        .get_function_decl(func)
        .signature
        .clone()
}

// Define a block with the same parameter and return types as the function
pub fn create_entry_block(fbuilder: &mut cl::FunctionBuilder<'_>) -> cl::Block {
    let block = fbuilder.create_block();
    fbuilder.seal_block(block);
    fbuilder.append_block_params_for_function_params(block);
    block
}

// fn main();
pub fn declare_main(module: &mut ObjectModule) -> FuncId {
    let call_conv = module.isa().default_call_conv();
    let mut sig = cl::Signature::new(call_conv);

    // Add the exit code return type
    sig.returns.push(cl::AbiParam::new(cl::types::I32));

    module
        .declare_function("main", Linkage::Export, &sig)
        .unwrap()
}
