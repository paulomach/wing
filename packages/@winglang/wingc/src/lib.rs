#![allow(clippy::all)]
#![deny(clippy::correctness)]
#![deny(clippy::suspicious)]
#![deny(clippy::complexity)]
#![allow(clippy::vec_box)]

#[macro_use]
extern crate lazy_static;

use ast::{Scope, Symbol};
use camino::{Utf8Path, Utf8PathBuf};
use closure_transform::ClosureTransformer;
use comp_ctx::set_custom_panic_hook;
use const_format::formatcp;
use diagnostic::{found_errors, report_diagnostic, Diagnostic, DiagnosticSeverity};
use dtsify::extern_dtsify::{is_extern_file, ExternDTSifier};
use file_graph::{File, FileGraph};
use files::Files;
use fold::Fold;
use generate_docs::generate_docs;
use indexmap::IndexMap;
use jsify::JSifier;

use lifting::LiftVisitor;
use parser::{as_wing_library, is_entrypoint_file, parse_wing_project};
use serde::Serialize;
use serde_json::Value;
use struct_schema::StructSchemaVisitor;
use type_check::jsii_importer::JsiiImportSpec;
use type_check::symbol_env::SymbolEnvKind;
use type_check::type_reference_transform::TypeReferenceTransformer;
use type_check_assert::TypeCheckAssert;
use valid_json_visitor::ValidJsonVisitor;
use visit::Visit;
use wasm_util::{ptr_to_str, string_to_combined_ptr, WASM_RETURN_ERROR};
use wingii::type_system::TypeSystem;

use crate::parser::normalize_path;
use std::alloc::{alloc, dealloc, Layout};

use std::{fs, mem};

use crate::ast::Phase;
use crate::type_check::symbol_env::SymbolEnv;
use crate::type_check::{SymbolEnvOrNamespace, TypeChecker, Types};

#[macro_use]
#[cfg(test)]
mod test_utils;

pub mod ast;
pub mod closure_transform;
mod comp_ctx;
pub mod debug;
pub mod diagnostic;
mod docs;
mod dtsify;
mod file_graph;
mod files;
pub mod fold;
pub mod generate_docs;
pub mod jsify;
pub mod json_schema_generator;
mod lifting;
pub mod lsp;
pub mod parser;
pub mod struct_schema;
mod ts_traversal;
pub mod type_check;
mod type_check_assert;
mod valid_json_visitor;
pub mod visit;
mod visit_context;
mod visit_stmt_before_super;
mod visit_types;
mod wasm_util;

const WINGSDK_ASSEMBLY_NAME: &'static str = "@winglang/sdk";

pub const WINGSDK_STD_MODULE: &'static str = "std";
const WINGSDK_CLOUD_MODULE: &'static str = "cloud";
const WINGSDK_UTIL_MODULE: &'static str = "util";
const WINGSDK_HTTP_MODULE: &'static str = "http";
const WINGSDK_MATH_MODULE: &'static str = "math";
const WINGSDK_AWS_MODULE: &'static str = "aws";
const WINGSDK_EXPECT_MODULE: &'static str = "expect";
const WINGSDK_FS_MODULE: &'static str = "fs";
const WINGSDK_SIM_MODULE: &'static str = "sim";
const WINGSDK_UI_MODULE: &'static str = "ui";

pub const UTIL_CLASS_NAME: &'static str = "Util";

const WINGSDK_BRINGABLE_MODULES: [&'static str; 9] = [
	WINGSDK_CLOUD_MODULE,
	WINGSDK_UTIL_MODULE,
	WINGSDK_HTTP_MODULE,
	WINGSDK_MATH_MODULE,
	WINGSDK_AWS_MODULE,
	WINGSDK_EXPECT_MODULE,
	WINGSDK_FS_MODULE,
	WINGSDK_SIM_MODULE,
	WINGSDK_UI_MODULE,
];

const WINGSDK_GENERIC: &'static str = "std.T1";
const WINGSDK_DURATION: &'static str = "std.Duration";
const WINGSDK_DATETIME: &'static str = "std.Datetime";
const WINGSDK_REGEX: &'static str = "std.Regex";
const WINGSDK_MAP: &'static str = "std.Map";
const WINGSDK_MUT_MAP: &'static str = "std.MutMap";
const WINGSDK_ARRAY: &'static str = "std.Array";
const WINGSDK_MUT_ARRAY: &'static str = "std.MutArray";
const WINGSDK_SET: &'static str = "std.Set";
const WINGSDK_MUT_SET: &'static str = "std.MutSet";
const WINGSDK_STRING: &'static str = "std.String";
const WINGSDK_JSON: &'static str = "std.Json";
const WINGSDK_MUT_JSON: &'static str = "std.MutJson";
const WINGSDK_RESOURCE: &'static str = "std.Resource";
const WINGSDK_IRESOURCE: &'static str = "std.IResource";
const WINGSDK_AUTOID_RESOURCE: &'static str = "std.AutoIdResource";
const WINGSDK_STRUCT: &'static str = "std.Struct";
const WINGSDK_TEST_CLASS_NAME: &'static str = "Test";
const WINGSDK_NODE: &'static str = "std.Node";
const WINGSDK_APP: &'static str = "std.IApp";

const WINGSDK_SIM_IRESOURCE: &'static str = "sim.IResource";
const WINGSDK_SIM_IRESOURCE_FQN: &'static str = formatcp!(
	"{assembly}.{iface}",
	assembly = WINGSDK_ASSEMBLY_NAME,
	iface = WINGSDK_SIM_IRESOURCE
);

const CONSTRUCT_BASE_CLASS: &'static str = "constructs.Construct";
const CONSTRUCT_BASE_INTERFACE: &'static str = "constructs.IConstruct";
const CONSTRUCT_NODE_PROPERTY: &'static str = "node";

const MACRO_REPLACE_SELF: &'static str = "$self$";
const MACRO_REPLACE_ARGS: &'static str = "$args$";
const MACRO_REPLACE_ARGS_TEXT: &'static str = "$args_text$";

pub const TRUSTED_LIBRARY_NPM_NAMESPACE: &'static str = "@winglibs";

pub const DEFAULT_PACKAGE_NAME: &'static str = "rootpkg";

#[derive(Serialize)]
pub struct CompilerOutput {
	imported_namespaces: Vec<String>,
}

/// Exposes an allocation function to the WASM host
///
/// _This implementation is copied from wasm-bindgen_
#[no_mangle]
pub unsafe extern "C" fn wingc_malloc(size: usize) -> *mut u8 {
	let align = mem::align_of::<usize>();
	let layout = Layout::from_size_align(size, align).expect("Invalid layout");
	if layout.size() > 0 {
		let ptr = alloc(layout);
		if !ptr.is_null() {
			return ptr;
		} else {
			std::alloc::handle_alloc_error(layout);
		}
	} else {
		return align as *mut u8;
	}
}

/// Expose a deallocation function to the WASM host
///
/// _This implementation is copied from wasm-bindgen_
#[no_mangle]
pub unsafe extern "C" fn wingc_free(ptr: *mut u8, size: usize) {
	// This happens for zero-length slices, and in that case `ptr` is
	// likely bogus so don't actually send this to the system allocator
	if size == 0 {
		return;
	}
	let align = mem::align_of::<usize>();
	let layout = Layout::from_size_align_unchecked(size, align);
	dealloc(ptr, layout);
}

/// Expose one time-initiliazation function to the WASM host,
/// should be called before any other function
#[no_mangle]
pub unsafe extern "C" fn wingc_init() {
	// Setup a custom panic hook to report panics as compilation diagnostics
	set_custom_panic_hook();
}

#[no_mangle]
pub unsafe extern "C" fn wingc_compile(ptr: u32, len: u32) -> u64 {
	let args = ptr_to_str(ptr, len);

	let split = args.split(";").collect::<Vec<&str>>();
	if split.len() != 2 {
		report_diagnostic(Diagnostic {
			message: format!("Expected 2 arguments to wingc_compile, got {}", split.len()),
			span: None,
			annotations: vec![],
			hints: vec![],
			severity: DiagnosticSeverity::Error,
		});
		return WASM_RETURN_ERROR;
	}
	let source_path = Utf8Path::new(split[0]);
	let output_dir = split.get(1).map(|s| Utf8Path::new(s)).expect("output dir not provided");

	if !source_path.exists() {
		report_diagnostic(Diagnostic {
			message: format!("Source path cannot be found: {}", source_path),
			span: None,
			annotations: vec![],
			hints: vec![],
			severity: DiagnosticSeverity::Error,
		});
		return WASM_RETURN_ERROR;
	}

	let results = compile(source_path, None, output_dir);

	if let Ok(results) = results {
		string_to_combined_ptr(serde_json::to_string(&results).unwrap())
	} else {
		WASM_RETURN_ERROR
	}
}

#[no_mangle]
pub unsafe extern "C" fn wingc_generate_docs(ptr: u32, len: u32) -> u64 {
	let args = ptr_to_str(ptr, len);
	let project_dir = Utf8Path::new(args);
	let results = generate_docs(project_dir);

	if let Ok(results) = results {
		string_to_combined_ptr(results)
	} else {
		WASM_RETURN_ERROR
	}
}

const LOCKFILES: [&'static str; 4] = ["pnpm-lock.yaml", "yarn.lock", "bun.lock", "bun.lockb"];

/// Wing sometimes can't find dependencies if they're installed with pnpm/yarn/bun.
/// Try to anticipate any issues that may arise from using pnpm/yarn/bun with winglibs
/// by emitting a warning if dependencies were installed with any of these package managers.
fn emit_warning_for_unsupported_package_managers(project_dir: &Utf8Path) {
	for lockfile in &LOCKFILES {
		let lockfile_path = project_dir.join(lockfile);
		if lockfile_path.exists() {
			report_diagnostic(Diagnostic {
				message: "The current project has a pnpm/yarn/bun lockfile. Wing hasn't been tested with package managers besides npm, so it may be unable to resolve dependencies to Wing libraries when using these tools. See https://github.com/winglang/wing/issues/6129 for more details.".to_string(),
				span: None,
				annotations: vec![],
				hints: vec![],
				severity: DiagnosticSeverity::Warning,
			});
		}
	}
}

pub fn type_check_file(
	scope: &mut Scope,
	types: &mut Types,
	file: &File,
	file_graph: &FileGraph,
	library_roots: &IndexMap<String, Utf8PathBuf>,
	jsii_types: &mut TypeSystem,
	jsii_imports: &mut Vec<JsiiImportSpec>,
) {
	let mut env = types.add_symbol_env(SymbolEnv::new(
		None,
		SymbolEnvKind::Scope,
		Phase::Preflight,
		0,
		file.package.clone(),
	));

	types.set_scope_env(scope, env);

	let mut tc = TypeChecker::new(types, file, file_graph, library_roots, jsii_types, jsii_imports);
	tc.add_jsii_module_to_env(
		&mut env,
		WINGSDK_ASSEMBLY_NAME.to_string(),
		vec![WINGSDK_STD_MODULE.to_string()],
		&Symbol::global(WINGSDK_STD_MODULE),
		None,
	);
	tc.add_builtins(scope);

	// If the file is an entrypoint file, we add "this" to its symbol environment
	if is_entrypoint_file(&file.path) {
		tc.add_this(&mut env);
	}

	tc.type_check_file_or_dir(scope);
}

/// Infer the root directory of the current Wing application or library.
///
/// Check the current file's directory for a wing.toml file or package.json file that has a "wing" field,
/// and continue searching up the directory tree until we find one.
/// If we run out of parent directories, fall back to the first directory we found.
pub fn find_nearest_wing_project_dir(source_path: &Utf8Path) -> Utf8PathBuf {
	let initial_dir: Utf8PathBuf = if source_path.is_dir() {
		source_path.to_owned()
	} else {
		source_path.parent().unwrap_or_else(|| Utf8Path::new("/")).to_owned()
	};
	let mut current_dir = initial_dir.as_path();
	loop {
		if current_dir.join("wing.toml").exists() {
			return current_dir.to_owned();
		}
		if current_dir.join("package.json").exists() {
			let package_json = fs::read_to_string(current_dir.join("package.json")).unwrap();
			if serde_json::from_str(&package_json).map_or(false, |v: Value| v.get("wing").is_some()) {
				return current_dir.to_owned();
			}
		}
		if current_dir == "/" {
			break;
		}
		current_dir = current_dir.parent().unwrap_or_else(|| Utf8Path::new("/"));
	}
	return initial_dir;
}

pub fn compile(source_path: &Utf8Path, source_text: Option<String>, out_dir: &Utf8Path) -> Result<CompilerOutput, ()> {
	let project_dir = find_nearest_wing_project_dir(source_path);
	let source_package = as_wing_library(&project_dir, false).unwrap_or_else(|| DEFAULT_PACKAGE_NAME.to_string());
	let source_path = normalize_path(source_path, None);
	let source_file = File::new(&source_path, source_package.clone());

	// A map from package names to their root directories
	let mut library_roots: IndexMap<String, Utf8PathBuf> = IndexMap::new();
	library_roots.insert(source_package, project_dir.to_owned());

	// -- PARSING PHASE --
	let mut files = Files::new();
	let mut file_graph = FileGraph::default();
	let mut tree_sitter_trees = IndexMap::new();
	let mut asts = IndexMap::new();
	let topo_sorted_files = parse_wing_project(
		&source_file,
		source_text,
		&mut files,
		&mut file_graph,
		&mut library_roots,
		&mut tree_sitter_trees,
		&mut asts,
	);

	emit_warning_for_unsupported_package_managers(&project_dir);

	// -- DESUGARING PHASE --

	// Transform all inflight closures defined in preflight into single-method resources
	let mut asts = asts
		.into_iter()
		.map(|(path, scope)| {
			let mut inflight_transformer = ClosureTransformer::new();
			let scope = inflight_transformer.fold_scope(scope);
			(path, scope)
		})
		.collect::<IndexMap<Utf8PathBuf, Scope>>();

	// -- TYPECHECKING PHASE --

	// Create universal types collection (need to keep this alive during entire compilation)
	let mut types = Types::new();
	let mut jsii_types = TypeSystem::new();

	// Create a universal JSII import spec (need to keep this alive during entire compilation)
	let mut jsii_imports = vec![];

	// Type check all files in topological order (start with files that don't bring any other
	// Wing files, then move on to files that depend on those, and repeat)
	for file in &topo_sorted_files {
		let mut scope = asts.swap_remove(&file.path).expect("matching AST not found");
		type_check_file(
			&mut scope,
			&mut types,
			&file,
			&file_graph,
			&mut library_roots,
			&mut jsii_types,
			&mut jsii_imports,
		);

		// Make sure all type reference are no longer considered references
		let mut tr_transformer = TypeReferenceTransformer { types: &mut types };
		let scope = tr_transformer.fold_scope(scope);

		// Validate the type checker didn't miss anything - see `TypeCheckAssert` for details
		let mut tc_assert = TypeCheckAssert::new(&types, found_errors());
		tc_assert.check(&scope);

		// Validate all Json literals to make sure their values are legal
		let mut json_checker = ValidJsonVisitor::new(&types);
		json_checker.check(&scope);

		asts.insert(file.path.to_owned(), scope);
	}

	let mut jsifier = JSifier::new(&mut types, &files, &file_graph, &source_path, &out_dir);

	// -- LIFTING PHASE --

	let mut asts = asts
		.into_iter()
		.map(|(path, scope)| {
			let mut lift = LiftVisitor::new(&jsifier);
			lift.visit_scope(&scope);
			(path, scope)
		})
		.collect::<IndexMap<Utf8PathBuf, Scope>>();

	// bail out now (before jsification) if there are errors (no point in jsifying)
	if found_errors() {
		return Err(());
	}

	// -- STRUCT SCHEMA GENERATION PHASE --
	// Need to do this before jsification so that we know what struct schemas need to be generated
	asts = asts
		.into_iter()
		.map(|(path, scope)| {
			let mut reference_visitor = StructSchemaVisitor::new(&path, &jsifier);
			reference_visitor.visit_scope(&scope);
			(path, scope)
		})
		.collect::<IndexMap<Utf8PathBuf, Scope>>();

	// -- JSIFICATION PHASE --

	for file in &topo_sorted_files {
		let scope = asts.get_mut(&file.path).expect("matching AST not found");
		jsifier.jsify(&file, &scope);
	}
	if !found_errors() {
		match jsifier.output_files.borrow().emit_files(out_dir) {
			Ok(()) => {}
			Err(err) => report_diagnostic(err.into()),
		}
	}

	// -- DTSIFICATION PHASE --
	if source_path.is_dir() {
		let preflight_file_map = jsifier.preflight_file_map.borrow();
		let dtsifier = dtsify::DTSifier::new(&mut types, &preflight_file_map, &mut file_graph);
		for file in &topo_sorted_files {
			let scope = asts.get_mut(&file.path).expect("matching AST not found");
			dtsifier.dtsify(&file, &scope);
		}
		if !found_errors() {
			let output_files = dtsifier.output_files.borrow();
			match output_files.emit_files(out_dir) {
				Ok(()) => {}
				Err(err) => report_diagnostic(err.into()),
			}
		}
	}

	// -- EXTERN DTSIFICATION PHASE --
	for source_files_env in &types.source_file_envs {
		if is_extern_file(source_files_env.0) {
			let mut extern_dtsifier = ExternDTSifier::new(&types);
			if !found_errors() {
				match extern_dtsifier.dtsify(source_files_env.0, source_files_env.1) {
					Ok(()) => {}
					Err(err) => report_diagnostic(err.into()),
				};
			}
		}
	}

	if found_errors() {
		return Err(());
	}

	let imported_namespaces = types
		.source_file_envs
		.iter()
		.filter_map(|(k, v)| match v {
			SymbolEnvOrNamespace::Namespace(_) => Some(k.to_string()),
			_ => None,
		})
		.collect::<Vec<String>>();

	Ok(CompilerOutput { imported_namespaces })
}

pub fn is_absolute_path(path: &Utf8Path) -> bool {
	if path.starts_with("/") {
		return true;
	}

	// Check if this is a Windows path instead by checking if the second char is a colon
	// Note: Cannot use Utf8Path::is_absolute() because it doesn't work with Windows paths on WASI
	let chars = path.as_str().chars().collect::<Vec<char>>();
	if chars.len() < 2 || chars[1] != ':' {
		return false;
	}

	return true;
}

#[cfg(test)]
mod sanity {
	use camino::{Utf8Path, Utf8PathBuf};

	use crate::{compile, diagnostic::assert_no_panics};
	use std::fs;

	fn get_wing_files<P>(dir: P) -> impl Iterator<Item = Utf8PathBuf>
	where
		P: AsRef<Utf8Path>,
	{
		fs::read_dir(dir.as_ref())
			.unwrap()
			.map(|entry| Utf8PathBuf::from_path_buf(entry.unwrap().path()).expect("invalid unicode path"))
			.filter(|path| path.is_file() && path.extension().map(|ext| ext == "w").unwrap_or(false))
	}

	fn compile_test(test_dir: &str, expect_failure: bool) {
		let test_dir = Utf8Path::new(test_dir).canonicalize_utf8().unwrap();
		for test_file in get_wing_files(&test_dir) {
			println!("\n=== {} ===\n", test_file);

			let out_dir = test_dir.join(format!("target/wingc/{}.out", test_file.file_name().unwrap()));

			// reset out_dir
			if out_dir.exists() {
				fs::remove_dir_all(&out_dir).expect("remove out dir");
			}

			let result = compile(&test_file, None, &out_dir);

			if result.is_err() {
				assert!(
					expect_failure,
					"{}: Expected compilation success, but failed: {:#?}",
					test_file,
					result.err().unwrap()
				);

				// Even if the test fails when we expect it to, none of the failures should be due to a compiler bug
				assert_no_panics();
			} else {
				assert!(
					!expect_failure,
					"{}: Expected compilation failure, but succeeded",
					test_file,
				);
			}
		}
	}

	#[test]
	fn can_compile_valid_files() {
		compile_test("../../../tests/valid", false);
	}

	#[test]
	fn can_compile_error_files() {
		compile_test("../../../tests/error", false);
	}

	#[test]
	fn cannot_compile_invalid_files() {
		compile_test("../../../tests/invalid", true);
	}
}
