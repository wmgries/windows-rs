use super::*;

#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord)]
pub struct GenericType {
    pub def: tables::TypeDef,
    pub generics: Vec<ElementType>,
}

impl GenericType {
    pub fn from_blob(blob: &mut Blob, generics: &[ElementType]) -> Self {
        blob.read_unsigned();
        // TODO: add "read_type_def_or_ref" method to Blob reader.
        let def =
            TypeDefOrRef::decode(blob.reader, blob.read_unsigned(), blob.file_index).resolve();
        let mut args = Vec::with_capacity(blob.read_unsigned() as usize);

        for _ in 0..args.capacity() {
            args.push(ElementType::from_blob(blob, generics));
        }

        Self {
            def,
            generics: args,
        }
    }

    pub fn from_type_def(def: tables::TypeDef, generics: Vec<ElementType>) -> Self {
        if generics.is_empty() {
            let generics = def
                .generics()
                .map(|generic| ElementType::GenericParam(generic))
                .collect();

            Self { def, generics }
        } else {
            Self { def, generics }
        }
    }

    pub fn bases(&self) -> impl Iterator<Item = Self> + '_ {
        self.def
            .bases()
            .map(|def| GenericType::from_type_def(def, Vec::new()))
    }

    pub fn interfaces2(&self) -> Vec<InterfaceInfo> {
        let mut result = Vec::new();

        fn add_interfaces(result: &mut Vec<InterfaceInfo>, parent: &GenericType, is_base: bool) {
            for child in parent.def.interfaces() {
                if let Some(def) = child.generic_interface(&parent.generics) {
                    if !result.iter().any(|info| info.def == def) {
                        add_interfaces(result, &def, is_base);

                        let kind = if child.is_default() {
                            InterfaceKind::Default
                        } else {
                            InterfaceKind::NonDefault
                        };

                        let version = def.def.version();

                        result.push(InterfaceInfo {
                            def,
                            kind,
                            is_base,
                            version,
                        });
                    }
                }
            }
        }

        add_interfaces(&mut result, self, false);

        for base in self.bases() {
            add_interfaces(&mut result, &base, true);
        }

        result
    }

    // TODO: remove and use interfaces2 in its place.
    pub fn interfaces(&self) -> impl Iterator<Item = (types::Interface, InterfaceKind)> + '_ {
        self.def.interfaces().filter_map(move |i| {
            let kind = if i.is_default() {
                InterfaceKind::Default
            } else {
                InterfaceKind::NonDefault
            };

            let interface = types::Interface(match i.interface() {
                TypeDefOrRef::TypeDef(def) => Self {
                    def,
                    generics: Vec::new(),
                },
                TypeDefOrRef::TypeRef(def) => {
                    if def.full_name() == ("Windows.Win32.Com", "IUnknown") {
                        return None;
                    }

                    Self {
                        def: def.resolve(),
                        generics: Vec::new(),
                    }
                }
                TypeDefOrRef::TypeSpec(def) => {
                    let mut blob = def.blob();
                    blob.read_unsigned();
                    Self::from_blob(&mut blob, &self.generics)
                }
            });

            Some((interface, kind))
        })
    }

    pub fn gen_name(&self, gen: &Gen) -> TokenStream {
        self.format_name(gen, to_ident)
    }

    pub fn gen_abi_name(&self, gen: &Gen) -> TokenStream {
        self.format_name(gen, to_abi_ident)
    }

    pub fn gen_guid(&self) -> TokenStream {
        if self.generics.is_empty() {
            let guid = self.def.guid().gen();

            quote! {
                ::windows::Guid::from_values(#guid)
            }
        } else {
            let tokens = self.gen_name(&Gen::Absolute);

            quote! {
                ::windows::Guid::from_signature(<#tokens as ::windows::RuntimeType>::SIGNATURE)
            }
        }
    }

    pub fn gen_signature(&self, signature: &str) -> TokenStream {
        let signature = Literal::byte_string(signature.as_bytes());

        if self.generics.is_empty() {
            return quote! { ::windows::ConstBuffer::from_slice(#signature) };
        }

        let generics = self.generics.iter().enumerate().map(|(index, g)| {
            let g = g.gen(&Gen::Absolute);
            let semi = if index != self.generics.len() - 1 {
                Some(quote! {
                    .push_slice(b";")
                })
            } else {
                None
            };

            quote! {
                .push_other(<#g as ::windows::RuntimeType>::SIGNATURE)
                #semi
            }
        });

        quote! {
            {
                ::windows::ConstBuffer::new()
                .push_slice(b"pinterface(")
                .push_slice(#signature)
                .push_slice(b";")
                #(#generics)*
                .push_slice(b")")
            }
        }
    }

    pub fn gen_phantoms(&self) -> TokenStream {
        TokenStream::from_iter(self.generics.iter().map(|g| {
            let g = g.gen(&Gen::Absolute);
            quote! { ::std::marker::PhantomData::<#g>, }
        }))
    }

    pub fn gen_constraints(&self) -> TokenStream {
        TokenStream::from_iter(self.generics.iter().map(|g| {
            let g = g.gen(&Gen::Absolute);
            quote! { #g: ::windows::RuntimeType + 'static, }
        }))
    }

    pub fn interface_signature(&self) -> String {
        let guid = self.def.guid();

        if self.generics.is_empty() {
            format!("{{{:#?}}}", guid)
        } else {
            let mut result = format!("pinterface({{{:#?}}}", guid);

            for generic in &self.generics {
                result.push(';');
                result.push_str(&generic.type_signature());
            }

            result.push(')');
            result
        }
    }

    fn format_name<F>(&self, gen: &Gen, format_name: F) -> TokenStream
    where
        F: FnOnce(&str) -> Ident,
    {
        let name = self.def.name();
        let namespace = gen.namespace(self.def.namespace());

        if self.generics.is_empty() {
            let name = format_name(name);
            quote! { #namespace#name }
        } else {
            let colon_separated = if namespace.as_str().is_empty() {
                quote! {}
            } else {
                quote! { :: }
            };

            let name = format_name(&name[..name.len() - 2]);
            let generics = self.generics.iter().map(|g| g.gen_name(gen));
            quote! { #namespace#name#colon_separated<#(#generics),*> }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generic() {
        let reader = TypeReader::get();
        let t = reader.resolve_type("Windows.Foundation", "IAsyncOperation`1");
        assert_eq!(
            t.gen_name(&Gen::Absolute).as_str(),
            "windows :: foundation :: IAsyncOperation :: < TResult >"
        );
        assert_eq!(
            t.gen_name(&Gen::Relative("Windows.Foundation")).as_str(),
            "IAsyncOperation < TResult >"
        );
    }
}
