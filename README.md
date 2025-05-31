# Cranelift Examples

The Cranelift documentation avoids talking about higher-level concepts or teaching about code generation in a more general sense. 

This repository is meant to provide tutorial-esque examples of tasks you'll encounter when trying to use Cranelift as a code generation backend for a compiler. 

Each example is structured to be either useful in isolation, or explicitly reference another example. I would recommend starting with [Outputting a binary](examples/output-a-binary/main.rs). As all other examples afterwards will use helpermethods which abstract away the boilerplate that it teaches. But other than that they're designed to be unordered.

[Outputting a binary](examples/output-a-binary/main.rs)\ 
[Lowering aggregate types such as Structs and Enums](examples/struct-and-enum/main.rs)
