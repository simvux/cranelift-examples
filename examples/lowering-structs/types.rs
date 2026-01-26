use cranelift::codegen::ir::ArgumentPurpose;
use cranelift::prelude as cl;
use cranelift_module::FuncId;
use std::collections::HashMap;

type Name = &'static str;

// While we won't be doing any type checking in this example, we still need to know the type of
// structs for the size and offsets.
#[derive(Clone, Copy, Debug)]
pub enum Type {
    Int,
    Struct(Name),
}

// Whether a struct will be passed as a pointer or as a set of independent values directly
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StructPassingMode {
    ByScalars,
    ByPointer,
}

/// We need to know the typing details of defined types and functions.
///
/// How exactly that should be provided will depend a lot on the rest of your compiler.
/// In this example we're gonna be using a hashmap of stringly identifiers to type data.
#[derive(Debug)]
pub struct LookupTable {
    struct_fields: HashMap<Name, Vec<(Name, Type)>>,
    function_types: HashMap<Name, (Vec<Type>, Type)>,
    pub function_names: HashMap<FuncId, Name>,
    ptr_size: u32,
}

impl LookupTable {
    /// Function signatures in Cranelift can look pretty different from the user-provided signature.
    ///
    /// Since Cranelift types/values can only represent primitives, a Struct will need to be passed
    /// either as multiple types/values or as a pointer implicitly.
    pub fn create_signature(&self, call_conv: cl::isa::CallConv, fname: &str) -> cl::Signature {
        // Get the type signatures from our source language
        let (fparams, fret) = self.function_types.get(fname).expect("function not found");

        // Buffers for the Cranelift type signature.
        let mut params = vec![];
        let mut returns = vec![];

        // If the return value is a large struct that's passed as pointer, instead of returning its
        // values directly, we use an out pointer as the first parameter. The callee will write
        // the result to that pointer, instead of returning directly through the return registers.
        match fret {
            Type::Int => returns.push(cl::AbiParam::new(cl::types::I32)),
            Type::Struct(name) => match self.struct_passing_mode(name) {
                StructPassingMode::ByScalars => {
                    self.for_scalars_of_struct(&mut |ty| returns.push(cl::AbiParam::new(ty)), name)
                }
                StructPassingMode::ByPointer => {
                    // The `ArgumentPurpose` is needed in-case our target architecture expects the
                    // out pointer to use a specific register.
                    let size_t = cl::Type::int_with_byte_size(self.ptr_size as u16).unwrap();
                    let param = cl::AbiParam::special(size_t, ArgumentPurpose::StructReturn);
                    params.push(param);
                }
            },
        };

        for p in fparams {
            match p {
                Type::Int => params.push(cl::AbiParam::new(cl::types::I32)),
                Type::Struct(name) => match self.struct_passing_mode(name) {
                    StructPassingMode::ByScalars => {
                        self.for_scalars_of_struct(
                            &mut |clty| params.push(cl::AbiParam::new(clty)),
                            name,
                        );
                    }
                    StructPassingMode::ByPointer => {
                        let size_t = cl::Type::int_with_byte_size(self.ptr_size as u16).unwrap();
                        params.push(cl::AbiParam::new(size_t));
                    }
                },
            }
        }

        cl::Signature {
            params,
            returns,
            call_conv,
        }
    }

    pub fn hardcoded(ptr_size: u32) -> Self {
        let function_types = [
            ("main", (vec![], Type::Int)),
            (
                "move_right",
                (
                    vec![Type::Struct("Player"), Type::Int],
                    Type::Struct("Player"),
                ),
            ),
        ]
        .into();

        let struct_fields = [
            (
                "Player",
                vec![("id", Type::Int), ("position", Type::Struct("Point"))],
            ),
            ("Point", vec![("x", Type::Int), ("y", Type::Int)]),
            ("unit", vec![]),
        ]
        .into();

        let function_names = HashMap::new();

        Self {
            ptr_size,
            function_names,
            function_types,
            struct_fields,
        }
    }

    fn for_scalars<F>(&self, f: &mut F, ty: Type)
    where
        F: FnMut(cl::Type),
    {
        match ty {
            Type::Int => f(cl::types::I32),
            Type::Struct(name) => self.for_scalars_of_struct(f, name),
        }
    }

    pub fn for_scalars_of_struct<F>(&self, f: &mut F, name: &str)
    where
        F: FnMut(cl::Type),
    {
        self.struct_fields
            .get(name)
            .expect("struct not found")
            .iter()
            .for_each(|&(_, ty)| self.for_scalars(f, ty))
    }

    pub fn return_type_of(&self, id: FuncId) -> Type {
        let fname = self.function_names[&id];
        self.function_types[fname].1
    }

    // If a struct fits in two registers, then avoid stack allocating it.
    pub fn struct_passing_mode(&self, name: &str) -> StructPassingMode {
        let mut scalars = 0;
        self.for_scalars_of_struct(&mut |_| scalars += 1, name);
        if scalars < 3 {
            StructPassingMode::ByScalars
        } else {
            StructPassingMode::ByPointer
        }
    }

    pub fn fields_of_struct(
        &self,
        name: &str,
    ) -> impl Iterator<Item = (usize, Name, Type)> + Clone {
        self.struct_fields
            .get(name)
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, &(name, ty))| (i, name, ty))
    }

    pub fn size_of_struct(&self, name: &str) -> u32 {
        let mut size = 0;
        self.for_scalars_of_struct(&mut |clty| size += clty.bytes(), name);
        size
    }

    pub fn size_of(&self, ty: Type) -> u32 {
        let mut size = 0;
        self.for_scalars(&mut |clty| size += clty.bytes(), ty);
        size
    }

    pub fn resolve_field(&self, type_: &str, field: &str) -> usize {
        self.struct_fields
            .get(type_)
            .expect("struct not found")
            .iter()
            .position(|(name, _)| *name == field)
            .expect("field not found")
    }

    pub fn type_of_field(&self, struct_: &str, field: usize) -> Type {
        self.struct_fields.get(struct_).expect("struct not found")[field].1
    }

    pub fn offset_of_field(&self, struct_: &str, field: usize) -> i32 {
        let fields = self.struct_fields.get(struct_).expect("struct not found");

        let mut offset = 0;
        for (i, (_, fty)) in fields.iter().enumerate() {
            if i == field {
                return offset;
            }

            offset += self.size_of(*fty) as i32;
        }

        panic!("field not found");
    }
}
