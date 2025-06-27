# Cranelift Examples

The Cranelift documentation avoids talking about higher-level concepts or teaching about code generation in a more general sense. 

This repository is meant to provide tutorial-esque examples of tasks you'll encounter when trying to use Cranelift as a code generation backend for a compiler. 

Each example is structured to be either useful in isolation, or explicitly reference another example. I would recommend starting with [Outputting a binary](examples/output-a-binary/main.rs). As all other examples afterwards will use helpermethods which abstract away the boilerplate that it teaches. But other than that they're designed to be unordered.

## Examples

* [Outputting a binary](examples/output-a-binary/main.rs)  
* [Representing Struct](examples/struct-layouts/main.rs)  
* [Representing Tagged Unions (`enum`)](examples/tagged-union-layouts/main.rs)  
* [Representing Dynamic Closures](examples/closures/main.rs)
* [Lowering aggregate types such as Structs](examples/lowering-structs/main.rs)

## Contributing

Try to follow these guidelines in your example: 

* Avoid including anything unrelated to what the example tries to show. 
* Heavily comment any part that is relevant to the example. 
* Use duplicated comments. The same comment being copy-pasted in two different functions for the same snippet is fine, as any function should make sense in isolation. 
* Use the boilerplate helpers provided by `lib.rs`
* Use a lot of `let my_ident = { ... };` to create a tree-like code style that mimics the kind of tree structures compilers often lower. 
* Avoid abstraction and wrappers over Cranelift
