// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use std::{fmt::Write as _, path::Path};

use oxc::{
    allocator::Allocator,
    ast::ast::Statement,
    codegen::{Codegen, CodegenOptions},
    mangler::{MangleOptions, Mangler},
    minifier::{CompressOptions, Compressor},
    parser::Parser,
    semantic::SemanticBuilder,
    span::SourceType,
    transformer::{TransformOptions, Transformer},
};

use super::Error;

pub(super) fn compile_scripts(scripts: &[(&str, String, bool)]) -> Result<String, Error> {
    if scripts.is_empty() {
        return Ok(String::new());
    }
    let mut source = String::new();
    for (path, script, is_typescript) in scripts {
        let allocator = Allocator::default();
        let source_type = SourceType::default()
            .with_module(true)
            .with_typescript(*is_typescript);
        let mut parsed = Parser::new(&allocator, script, source_type).parse();
        if !parsed.diagnostics.is_empty() {
            return Err(Error::JavaScript {
                path: (*path).to_owned(),
                message: format!("parser diagnostics: {:?}", parsed.diagnostics),
            });
        }
        if has_module_syntax(&parsed.program) {
            return Err(Error::ModuleSyntax {
                path: (*path).to_owned(),
            });
        }
        let semantic = SemanticBuilder::new()
            .with_check_syntax_error(true)
            .build(&parsed.program);
        if !semantic.diagnostics.is_empty() {
            return Err(Error::JavaScript {
                path: (*path).to_owned(),
                message: format!("semantic diagnostics: {:?}", semantic.diagnostics),
            });
        }
        let transformed =
            Transformer::new(&allocator, Path::new(path), &TransformOptions::default())
                .build_with_scoping(semantic.semantic.into_scoping(), &mut parsed.program);
        if !transformed.diagnostics.is_empty() {
            return Err(Error::JavaScript {
                path: (*path).to_owned(),
                message: format!("transformer diagnostics: {:?}", transformed.diagnostics),
            });
        }
        #[allow(deprecated)]
        let helpers_required = !transformed.helpers_used.is_empty();
        if helpers_required {
            return Err(Error::JavaScript {
                path: (*path).to_owned(),
                message: "transformation requires unsupported runtime helpers".to_owned(),
            });
        }
        let transformed_source = Codegen::new()
            .with_options(CodegenOptions::minify())
            .build(&parsed.program)
            .code;
        write!(
            source,
            "(function(){{\"use strict\";{transformed_source}}})();"
        )
        .expect("writing to String cannot fail");
    }
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &source, SourceType::default().with_script(true)).parse();
    if !parsed.diagnostics.is_empty() {
        return Err(Error::JavaScript {
            path: scripts[0].0.to_owned(),
            message: format!("{:?}", parsed.diagnostics),
        });
    }
    let mut program = parsed.program;
    let semantic = SemanticBuilder::new()
        .with_check_syntax_error(true)
        .build(&program);
    if !semantic.diagnostics.is_empty() {
        return Err(Error::JavaScript {
            path: scripts[0].0.to_owned(),
            message: format!("{:?}", semantic.diagnostics),
        });
    }
    let stats = semantic.semantic.stats();
    let scoping = semantic.semantic.into_scoping();
    Compressor::new(&allocator).build_with_scoping(
        &mut program,
        scoping,
        CompressOptions::default(),
    );
    let mangled = Mangler::new()
        .with_options(MangleOptions {
            top_level: Some(false),
            ..Default::default()
        })
        .with_stats(stats)
        .build(&program);
    Ok(Codegen::new()
        .with_options(CodegenOptions::minify())
        .with_scoping(Some(mangled.scoping))
        .with_private_member_mappings(Some(mangled.class_private_mappings))
        .build(&program)
        .code)
}

fn has_module_syntax(program: &oxc::ast::ast::Program<'_>) -> bool {
    program.body.iter().any(|statement| {
        statement.is_module_declaration()
            || matches!(statement, Statement::TSImportEqualsDeclaration(_))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxc::{allocator::Allocator, parser::Parser, span::SourceType};

    #[test]
    fn typescript_is_erased_into_executable_classic_javascript() {
        let script = compile_scripts(&[(
            "src/components/time.ts",
            "type Value = string; interface Item { value: Value } const identity = <T>(value: T): T => value; const item: Item = { value: identity<string>('ok') }; document.querySelectorAll('[data-relative-time]').forEach((element: Element) => { (element as HTMLElement).dataset.unit = item.value; });".to_owned(),
            true,
        )])
        .unwrap();
        assert!(!script.contains("interface") && !script.contains("type Value"));
        assert!(script.contains("querySelectorAll") && script.contains("dataset.unit"));
        let allocator = Allocator::default();
        let parsed =
            Parser::new(&allocator, &script, SourceType::default().with_script(true)).parse();
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    }

    #[test]
    fn scripts_are_isolated_minified_and_preserve_dom_properties() {
        let script = compile_scripts(&[
            ("src/a/one.js", "const value = 1; document.querySelectorAll('[data-relative-time]').forEach(el => el.dataset.unit = value);".to_owned(), false),
            ("src/z/two.js", "const value = 2; window.second = value;".to_owned(), false),
        ])
        .unwrap();
        assert!(script.starts_with("!function()") || script.starts_with("(function()"));
        assert!(
            script.contains("querySelectorAll")
                && script.contains("dataset")
                && script.contains(".unit")
        );
        assert!(script.len() < 180);
    }

    #[test]
    fn rejects_invalid_and_module_scripts_with_path_diagnostics() {
        assert!(matches!(
            compile_scripts(&[("src/bad.ts", "const value: = 1;".to_owned(), true)]),
            Err(Error::JavaScript { path, .. }) if path == "src/bad.ts"
        ));
        assert!(matches!(
            compile_scripts(&[("src/bad.js", "const =;".to_owned(), false)]),
            Err(Error::JavaScript { path, .. }) if path == "src/bad.js"
        ));
        for source in ["import value from 'value';", "export const value = 1;"] {
            assert!(matches!(
                compile_scripts(&[("src/bad.js", source.to_owned(), false)]),
                Err(Error::ModuleSyntax { path }) if path == "src/bad.js"
            ));
        }
        assert!(matches!(
            compile_scripts(&[("src/bad.ts", "import type { Value } from 'value';".to_owned(), true)]),
            Err(Error::ModuleSyntax { path }) if path == "src/bad.ts"
        ));
    }
}
