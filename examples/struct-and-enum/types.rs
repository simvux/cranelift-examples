use cranelift::codegen::ir::ArgumentPurpose;
use cranelift::prelude as cl;
use std::collections::HashMap;

type Name = &'static str;

// While we won't be doing any type checking in this example, we still need to know the type of
// structs for the size and offsets.
#[derive(Clone, Copy)]
pub enum Type {
    Int,
    Struct(Name),
}

impl Type {
    pub fn unit() -> Type {
        Type::Struct("unit")
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StructPassingMode {
    ByScalars,
    ByPointer,
}

/// Lookup tables for our defined types
pub struct TypeResolver {
    struct_fields: HashMap<Name, Vec<(Name, Type)>>,
    ptr_size: u32,
}

impl TypeResolver {
    /// Function signatures in Cranelift can look pretty different from the user-provided signature.
    ///
    /// Since Cranelift types/values can only represent primitives, a Struct will need to be passed
    /// either as multiple types/values or implicitly as a pointer.
    pub fn create_signature(
        &self,
        call_conv: cl::isa::CallConv,
        fparams: &[Type],
        fret: Type,
    ) -> cl::Signature {
        let mut params = vec![];

        let returns = match fret {
            Type::Int => vec![cl::AbiParam::new(cl::types::I64)],
            Type::Struct(name) => match self.struct_passing_mode(name) {
                StructPassingMode::ByScalars => self.fold_scalars_of_struct(
                    vec![],
                    &mut |mut buf, ty| {
                        buf.push(cl::AbiParam::new(ty));
                        buf
                    },
                    name,
                ),
                StructPassingMode::ByPointer => {
                    let ty = cl::Type::int_with_byte_size(self.ptr_size as u16).unwrap();
                    let param = cl::AbiParam::special(ty, ArgumentPurpose::StructReturn);
                    params.push(param);
                    vec![]
                }
            },
        };

        for p in fparams {
            match p {
                Type::Int => params.push(cl::AbiParam::new(cl::types::I64)),
                Type::Struct(name) => match self.struct_passing_mode(name) {
                    StructPassingMode::ByScalars => todo!(),
                    StructPassingMode::ByPointer => todo!(),
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
        let struct_fields = [
            (
                "Player",
                vec![("id", Type::Int), ("position", Type::Struct("Point"))],
            ),
            ("Point", vec![("x", Type::Int), ("y", Type::Int)]),
            ("unit", vec![]),
        ]
        .into();

        Self {
            ptr_size,
            struct_fields,
        }
    }

    fn fold_scalars<T, F>(&self, acc: T, f: &mut F, ty: Type) -> T
    where
        F: FnMut(T, cl::Type) -> T,
    {
        match ty {
            Type::Int => f(acc, cl::types::I64),
            Type::Struct(name) => self.fold_scalars_of_struct(acc, f, name),
        }
    }

    pub fn fold_scalars_of_struct<T, F>(&self, acc: T, f: &mut F, name: &str) -> T
    where
        F: FnMut(T, cl::Type) -> T,
    {
        self.struct_fields
            .get(name)
            .expect("struct not found")
            .iter()
            .fold(acc, move |acc, &(_, ty)| self.fold_scalars(acc, f, ty))
    }

    // If a struct fits in two registers, then avoid stack allocating it.
    pub fn struct_passing_mode(&self, name: &str) -> StructPassingMode {
        if self.fold_scalars_of_struct(0, &mut |n, _| n + 1, name) > 2 {
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
        self.fold_scalars_of_struct(0, &mut |n, clty| n + clty.bytes(), name)
    }

    pub fn size_of(&self, ty: Type) -> u32 {
        self.fold_scalars(0, &mut |n, clty| n + clty.bytes(), ty)
    }

    pub fn resolve_field(&self, type_: &str, field: &str) -> usize {
        self.struct_fields
            .get(type_)
            .expect("struct not found")
            .iter()
            .position(|(name, _)| *name == field)
            .expect("field not found")
    }

    fn name_of_field(&self, type_: &str, field: usize) -> Name {
        self.struct_fields[type_][field].0
    }

    pub fn size_of_field(&self, struct_: &str, field: usize) -> u32 {
        let fty = self
            .struct_fields
            .get(struct_)
            .expect("struct not found")
            .get(field)
            .expect("field not found")
            .1;

        self.size_of(fty)
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
