use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn, Meta};

#[proc_macro_attribute]
pub fn test(args: TokenStream, input: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(input as ItemFn);
    let fn_name = &input_fn.sig.ident;
    let fn_name_str = fn_name.to_string();
    let is_async = input_fn.sig.asyncness.is_some();

    // Parse attributes for should_panic
    let should_panic = if !args.is_empty() {
        let meta = parse_macro_input!(args as Meta);
        matches!(meta, Meta::Path(path) if path.is_ident("should_panic"))
    } else {
        false
    };

    // Use module path to create unique test name
    let test_name_with_module = format!("{}::{}", module_path!(), fn_name_str);

    // Generate simple wrapper function name
    let wrapper_fn_name = syn::Ident::new(
        &format!("__vampire_test_wrapper_{}", fn_name_str),
        fn_name.span(),
    );

    // On non-Android platforms, passthrough to standard test attributes
    let non_android_impl = if is_async {
        quote! {
            #[cfg(not(target_os = "android"))]
            #[tokio::test]
            #input_fn
        }
    } else {
        quote! {
            #[cfg(not(target_os = "android"))]
            #[test]
            #input_fn
        }
    };

    // Register test entry with metadata and function pointer
    let test_registration = quote! {
        #[cfg(target_os = "android")]
        ::vampire::inventory::submit! {
            ::vampire::TestEntry {
                metadata: ::vampire::TestMetadata {
                    name: #test_name_with_module,
                    r#async: #is_async,
                    should_panic: #should_panic,
                },
                test_fn: #wrapper_fn_name,
            }
        }
    };

    let wrapper_impl = if is_async {
        // Async test wrapper
        quote! {
            #[cfg(target_os = "android")]
            fn #wrapper_fn_name() -> bool {
                let result = std::panic::catch_unwind(|| {
                    let runtime = tokio::runtime::Runtime::new().unwrap();
                    runtime.block_on(#fn_name())
                });

                match result {
                    Ok(_) => !#should_panic,
                    Err(_) => #should_panic,
                }
            }
        }
    } else {
        // Sync test wrapper
        quote! {
            #[cfg(target_os = "android")]
            fn #wrapper_fn_name() -> bool {
                let result = std::panic::catch_unwind(|| {
                    #fn_name()
                });

                match result {
                    Ok(_) => !#should_panic,
                    Err(_) => #should_panic,
                }
            }
        }
    };

    // Keep the original function, add the wrapper, and register metadata
    let output = quote! {
        #[cfg(target_os = "android")]
        #input_fn

        #non_android_impl

        #wrapper_impl

        #test_registration
    };

    output.into()
}
