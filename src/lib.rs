use clap::{arg, command};
use cranelift::prelude::*;
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::path::PathBuf;
use std::{fs::File, io::Write};

/// Performs initialization and finalization of cranelift similarly to the instructions provided in [output-a-binary](examples/output-a-binary/main.rs)
pub fn skip_boilerplate(
    unit_name: &[u8],
    f: impl FnOnce(
        &mut codegen::Context,
        &mut FunctionBuilderContext,
        &mut ObjectModule,
        clap::ArgMatches,
    ),
) {
    let args = command!()
        .arg(arg!(-t --target-triple "Target triple arch-vendor-platform"))
        .arg(arg!(-o --output "Path for output object file"))
        .get_matches();

    let isa = {
        let mut builder = settings::builder();

        builder.set("opt_level", "none").unwrap();

        let flags = settings::Flags::new(builder);

        let triple = args
            .get_one::<&str>("target-triple")
            .unwrap_or_else(|| &"x86_64-unknown-linux");

        isa::lookup_by_name(triple).unwrap().finish(flags).unwrap()
    };

    let mut module = {
        let libcall_names = cranelift_module::default_libcall_names();
        let builder = ObjectBuilder::new(isa.clone(), unit_name, libcall_names).unwrap();
        ObjectModule::new(builder)
    };

    let path: Option<PathBuf> = args.get_one("output").cloned();

    let mut ctx = codegen::Context::new();
    let mut fctx = FunctionBuilderContext::new();

    f(&mut ctx, &mut fctx, &mut module, args);

    let product = module.finish();

    match path {
        Some(path) => {
            let bytes = product.emit().unwrap();

            let mut f = File::create(&path).unwrap();
            f.write_all(&bytes).unwrap();

            println!(" wrote output to {} ", path.display());
        }
        None => {
            println!(" no `-o` path specified ");
        }
    }
}

pub fn define_main_function() {}
