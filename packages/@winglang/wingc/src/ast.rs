use std::fmt::{Debug, Display};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};

use camino::Utf8PathBuf;
use indexmap::{Equivalent, IndexMap};
use itertools::Itertools;

use crate::diagnostic::WingSpan;

use crate::type_check::CLOSURE_CLASS_HANDLE_METHOD;

static EXPR_COUNTER: AtomicUsize = AtomicUsize::new(0);
static SCOPE_COUNTER: AtomicUsize = AtomicUsize::new(0);
static ARGLIST_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Eq, Clone)]
pub struct Symbol {
	pub name: String,
	pub span: WingSpan,
}

impl Symbol {
	pub fn new<S: Into<String>>(name: S, span: WingSpan) -> Self {
		Self {
			name: name.into(),
			span,
		}
	}

	pub fn global<S: Into<String>>(name: S) -> Self {
		Self {
			name: name.into(),
			span: Default::default(),
		}
	}

	/// Returns true if the symbols refer to the same name AND location in the source code.
	/// Use `eq` to compare symbols only by name.
	pub fn same(&self, other: &Self) -> bool {
		self.name == other.name && self.span == other.span
	}
}

impl Ord for Symbol {
	fn cmp(&self, other: &Self) -> std::cmp::Ordering {
		self.name.cmp(&other.name).then(self.span.cmp(&other.span))
	}
}

impl PartialOrd for Symbol {
	fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
		Some(self.cmp(other))
	}
}

impl Hash for Symbol {
	fn hash<H: Hasher>(&self, state: &mut H) {
		self.name.hash(state);
	}
}

impl PartialEq for Symbol {
	fn eq(&self, other: &Self) -> bool {
		self.name == other.name
	}
}

impl Display for Symbol {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{}", self.name)
	}
}

impl Equivalent<Symbol> for str {
	fn equivalent(&self, key: &Symbol) -> bool {
		self == key.name
	}
}

impl From<&str> for Symbol {
	fn from(s: &str) -> Self {
		Symbol::global(s)
	}
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
	Inflight,
	Preflight,
	Independent,
}

impl Phase {
	/// Returns true if the current phase can call into given phase.
	/// Rules:
	/// - Independent functions can be called from any phase.
	/// - Preflight can call into preflight
	/// - Inflight can call into inflight
	///
	pub fn can_call_to(&self, to: &Phase) -> bool {
		match to {
			Phase::Independent => true,
			Phase::Inflight | Phase::Preflight => to == self,
		}
	}
}

impl Display for Phase {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Phase::Inflight => write!(f, "inflight"),
			Phase::Preflight => write!(f, "preflight"),
			Phase::Independent => write!(f, "independent"),
		}
	}
}

#[derive(Debug, Clone)]
pub struct TypeAnnotation {
	pub kind: TypeAnnotationKind,
	pub span: WingSpan,
}

#[derive(Debug, Clone)]
pub enum TypeAnnotationKind {
	Inferred,
	Number,
	String,
	Bool,
	Duration,
	Datetime,
	Regex,
	Void,
	Json,
	MutJson,
	Optional(Box<TypeAnnotation>),
	Array(Box<TypeAnnotation>),
	MutArray(Box<TypeAnnotation>),
	Map(Box<TypeAnnotation>),
	MutMap(Box<TypeAnnotation>),
	Set(Box<TypeAnnotation>),
	MutSet(Box<TypeAnnotation>),
	Function(FunctionSignature),
	UserDefined(UserDefinedType),
}

// In the future this may be an enum for type-alias, class, etc. For now its just a nested name.
// Also this root,fields thing isn't really useful, should just turn in to a Vec<Symbol>.
#[derive(Debug, Clone, Eq)]
pub struct UserDefinedType {
	pub root: Symbol,
	pub fields: Vec<Symbol>,
	pub span: WingSpan,
}

impl Hash for UserDefinedType {
	fn hash<H: Hasher>(&self, state: &mut H) {
		self.root.hash(state);
		self.fields.hash(state);
	}
}

impl PartialEq for UserDefinedType {
	fn eq(&self, other: &Self) -> bool {
		self.root == other.root && self.fields == other.fields
	}
}

impl UserDefinedType {
	pub fn for_class(class: &Class) -> Self {
		Self {
			root: class.name.clone(),
			fields: vec![],
			span: class.name.span.clone(),
		}
	}

	pub fn full_path(&self) -> Vec<Symbol> {
		let mut path = vec![self.root.clone()];
		path.extend(self.fields.clone());
		path
	}

	pub fn full_path_str(&self) -> String {
		self.full_path().iter().join(".")
	}

	pub fn field_path_str(&self) -> String {
		self.fields.iter().join(".")
	}
}

impl Display for UserDefinedType {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{}", self.full_path_str())
	}
}

impl Display for TypeAnnotationKind {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			TypeAnnotationKind::Inferred => write!(f, "inferred"),
			TypeAnnotationKind::Number => write!(f, "num"),
			TypeAnnotationKind::String => write!(f, "str"),
			TypeAnnotationKind::Bool => write!(f, "bool"),
			TypeAnnotationKind::Duration => write!(f, "duration"),
			TypeAnnotationKind::Datetime => write!(f, "datetime"),
			TypeAnnotationKind::Regex => write!(f, "regex"),
			TypeAnnotationKind::Void => write!(f, "void"),
			TypeAnnotationKind::Json => write!(f, "Json"),
			TypeAnnotationKind::MutJson => write!(f, "MutJson"),
			TypeAnnotationKind::Optional(t) => write!(f, "{}?", t),
			TypeAnnotationKind::Array(t) => write!(f, "Array<{}>", t),
			TypeAnnotationKind::MutArray(t) => write!(f, "MutArray<{}>", t),
			TypeAnnotationKind::Map(t) => write!(f, "Map<{}>", t),
			TypeAnnotationKind::MutMap(t) => write!(f, "MutMap<{}>", t),
			TypeAnnotationKind::Set(t) => write!(f, "Set<{}>", t),
			TypeAnnotationKind::MutSet(t) => write!(f, "MutSet<{}>", t),
			TypeAnnotationKind::Function(t) => write!(f, "{}", t),
			TypeAnnotationKind::UserDefined(user_defined_type) => write!(f, "{}", user_defined_type),
		}
	}
}

impl Display for TypeAnnotation {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		std::fmt::Display::fmt(&self.kind, f)
	}
}

impl Display for FunctionSignature {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let phase_str = match self.phase {
			Phase::Inflight => "inflight ",
			Phase::Preflight => "preflight ",
			Phase::Independent => "",
		};
		let params_str = self
			.parameters
			.iter()
			.map(|a| {
				if a.name.name.is_empty() {
					format!("{}", a.type_annotation)
				} else {
					format!("{}: {}", a.name, a.type_annotation)
				}
			})
			.collect::<Vec<String>>()
			.join(", ");

		let ret_type_str = format!("{}", &self.return_type);
		write!(f, "{phase_str}({params_str}): {ret_type_str}")
	}
}

#[derive(Debug, Clone)]
pub struct FunctionSignature {
	pub parameters: Vec<FunctionParameter>,
	pub return_type: Box<TypeAnnotation>,
	pub phase: Phase,
}

impl FunctionSignature {
	pub fn to_type_annotation(&self) -> TypeAnnotation {
		TypeAnnotation {
			kind: TypeAnnotationKind::Function(self.clone()),
			// Function signatures may not necessarily have spans
			span: Default::default(),
		}
	}
}

#[derive(Debug, Clone)]
pub struct FunctionParameter {
	pub name: Symbol,
	pub type_annotation: TypeAnnotation,
	pub reassignable: bool,
	pub variadic: bool,
}

#[derive(Debug)]
pub enum FunctionBody {
	/// The function body implemented within a Wing scope.
	Statements(Scope),
	/// The `extern` modifier value, pointing to an external implementation file
	External(Utf8PathBuf),
}

#[derive(Debug)]
pub struct FunctionDefinition {
	/// The name of the function ('None' if this is a closure).
	pub name: Option<Symbol>,
	/// The function implementation.
	pub body: FunctionBody,
	/// The function signature, including the return type.
	pub signature: FunctionSignature,
	/// Whether this function is static or not. In case of a closure, this is always true.
	pub is_static: bool,
	/// Function's access modifier. In case of a closure, this is always public.
	pub access: AccessModifier,
	/// Function's documentation
	pub doc: Option<String>,
	pub span: WingSpan,
}

#[derive(Debug)]
pub struct Stmt {
	pub kind: StmtKind,
	pub span: WingSpan,
	pub idx: usize,
	pub doc: Option<String>,
}

#[derive(Debug)]
pub struct ElseIfBlock {
	pub condition: Expr,
	pub statements: Scope,
}

#[derive(Debug)]
pub struct ElseIfLetBlock {
	pub reassignable: bool,
	pub var_name: Symbol,
	pub value: Expr,
	pub statements: Scope,
}

#[derive(Debug)]
pub struct Class {
	pub name: Symbol,
	pub span: WingSpan,
	pub fields: Vec<ClassField>,
	pub methods: Vec<(Symbol, FunctionDefinition)>,
	pub initializer: FunctionDefinition,
	pub inflight_initializer: FunctionDefinition,
	pub parent: Option<UserDefinedType>, // base class (the expression is a reference to a user defined type)
	pub implements: Vec<UserDefinedType>,
	pub phase: Phase,
	pub access: AccessModifier,
	pub auto_id: bool,
}

impl Class {
	/// Returns all methods, including the initializer and inflight initializer.
	pub fn all_methods(&self, include_initializers: bool) -> Vec<&FunctionDefinition> {
		let mut methods: Vec<&FunctionDefinition> = vec![];

		for (_, m) in &self.methods {
			methods.push(&m);
		}

		if include_initializers {
			methods.push(&self.initializer);
			methods.push(&self.inflight_initializer);
		}

		methods
	}

	pub fn inflight_methods(&self, include_initializers: bool) -> Vec<&FunctionDefinition> {
		self
			.all_methods(include_initializers)
			.iter()
			.filter(|m| m.signature.phase == Phase::Inflight)
			.map(|f| *f)
			.collect_vec()
	}

	pub fn inflight_fields(&self) -> Vec<&ClassField> {
		self.fields.iter().filter(|f| f.phase == Phase::Inflight).collect_vec()
	}

	/// Returns the function definition of the "handle" method of this class (if this is a closure
	/// class). Otherwise returns None.
	pub fn closure_handle_method(&self) -> Option<&FunctionDefinition> {
		for method in self.inflight_methods(false) {
			if let Some(name) = &method.name {
				if name.name == CLOSURE_CLASS_HANDLE_METHOD {
					return Some(method);
				}
			}
		}

		None
	}

	pub fn preflight_methods(&self, include_initializers: bool) -> Vec<&FunctionDefinition> {
		self
			.all_methods(include_initializers)
			.iter()
			.filter(|f| f.signature.phase != Phase::Inflight)
			.map(|f| *f)
			.collect_vec()
	}
}

#[derive(Debug)]
pub struct Interface {
	pub name: Symbol,
	// Each method has a symbol, a signature, and an optional documentation string
	pub methods: Vec<(Symbol, FunctionSignature, Option<String>)>,
	pub extends: Vec<UserDefinedType>,
	pub access: AccessModifier,
	pub phase: Phase,
}

#[derive(Debug)]
pub struct Struct {
	pub name: Symbol,
	pub extends: Vec<UserDefinedType>,
	pub fields: Vec<StructField>,
	pub access: AccessModifier,
}

#[derive(Debug)]
pub struct Enum {
	pub name: Symbol,
	// Each value has a symbol and an optional documenation string
	pub values: IndexMap<Symbol, Option<String>>,
	pub access: AccessModifier,
}

#[derive(Debug)]
pub enum BringSource {
	BuiltinModule(Symbol),
	/// The name of the trusted module, and the path to the library (usually inside node_modules)
	TrustedModule(Symbol, Utf8PathBuf),
	/// The name of the library, and the path to the library (usually inside node_modules)
	WingLibrary(Symbol, Utf8PathBuf),
	JsiiModule(Symbol),
	/// Refers to a relative path to a file
	WingFile(Utf8PathBuf),
	/// Refers to a relative path to a directory
	Directory(Utf8PathBuf),
}

#[derive(Debug)]
pub enum AssignmentKind {
	Assign,
	AssignIncr,
	AssignDecr,
}

#[derive(Debug)]
pub struct IfLet {
	pub reassignable: bool,
	pub var_name: Symbol,
	pub value: Expr,
	pub statements: Scope,
	pub else_if_statements: Vec<ElseIfs>,
	pub else_statements: Option<Scope>,
}

#[derive(Debug)]
pub enum ElseIfs {
	ElseIfBlock(ElseIfBlock),
	ElseIfLetBlock(ElseIfLetBlock),
}

#[derive(Debug)]
pub enum StmtKind {
	Bring {
		source: BringSource,
		identifier: Option<Symbol>,
	},
	SuperConstructor {
		arg_list: ArgList,
	},
	Let {
		reassignable: bool,
		var_name: Symbol,
		initial_value: Expr,
		type_: Option<TypeAnnotation>,
	},
	ForLoop {
		iterator: Symbol,
		iterable: Expr,
		statements: Scope,
	},
	While {
		condition: Expr,
		statements: Scope,
	},
	IfLet(IfLet),
	If {
		condition: Expr,
		statements: Scope,
		else_if_statements: Vec<ElseIfBlock>,
		else_statements: Option<Scope>,
	},
	Break,
	Continue,
	Return(Option<Expr>),
	Throw(Expr),
	Expression(Expr),
	Assignment {
		kind: AssignmentKind,
		variable: Reference,
		value: Expr,
	},
	Scope(Scope),
	Class(Class),
	Interface(Interface),
	Struct(Struct),
	Enum(Enum),
	TryCatch {
		try_statements: Scope,
		catch_block: Option<CatchBlock>,
		finally_statements: Option<Scope>,
	},
	ExplicitLift(ExplicitLift),
}

impl StmtKind {
	pub fn is_type_def(&self) -> bool {
		matches!(
			self,
			StmtKind::Class(_) | StmtKind::Interface(_) | StmtKind::Struct(_) | StmtKind::Enum(_)
		)
	}
}

#[derive(Debug)]
pub struct ExplicitLift {
	pub qualifications: Vec<LiftQualification>,
	pub statements: Scope,
}

#[derive(Debug)]
pub struct LiftQualification {
	pub obj: Expr,
	pub ops: Vec<Symbol>,
}

#[derive(Debug)]
pub struct CatchBlock {
	pub statements: Scope,
	pub exception_var: Option<Symbol>,
}

#[derive(Debug)]
pub struct ClassField {
	pub name: Symbol,
	pub member_type: TypeAnnotation,
	pub reassignable: bool,
	pub phase: Phase,
	pub is_static: bool,
	pub access: AccessModifier,
	pub doc: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AccessModifier {
	Private,
	Public,
	Protected,
	Internal,
}

impl Display for AccessModifier {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			AccessModifier::Private => write!(f, "private"),
			AccessModifier::Public => write!(f, "public"),
			AccessModifier::Protected => write!(f, "protected"),
			AccessModifier::Internal => write!(f, "internal"),
		}
	}
}

#[derive(Debug)]
pub struct StructField {
	pub name: Symbol,
	pub member_type: TypeAnnotation,
	pub doc: Option<String>,
}

#[derive(Debug)]
pub struct Intrinsic {
	pub name: Symbol,
	pub arg_list: Option<ArgList>,
	pub kind: IntrinsicKind,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum IntrinsicKind {
	/// Error state
	Unknown,
	Dirname,
	Filename,
	App,
}

impl Display for IntrinsicKind {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			IntrinsicKind::Unknown => write!(f, "@"),
			IntrinsicKind::Dirname => write!(f, "@dirname"),
			IntrinsicKind::Filename => write!(f, "@filename"),
			IntrinsicKind::App => write!(f, "@app"),
		}
	}
}

impl IntrinsicKind {
	pub fn from_str(s: &str) -> Self {
		match s {
			"@dirname" => IntrinsicKind::Dirname,
			"@filename" => IntrinsicKind::Filename,
			"@app" => IntrinsicKind::App,
			_ => IntrinsicKind::Unknown,
		}
	}

	pub fn is_valid_phase(&self, phase: &Phase) -> bool {
		match self {
			IntrinsicKind::Unknown => true,
			IntrinsicKind::Dirname => match phase {
				Phase::Preflight => true,
				_ => false,
			},
			IntrinsicKind::Filename => match phase {
				Phase::Preflight => true,
				_ => false,
			},
			IntrinsicKind::App => match phase {
				Phase::Preflight => true,
				_ => false,
			},
		}
	}
}

impl Into<Symbol> for IntrinsicKind {
	fn into(self) -> Symbol {
		Symbol::global(self.to_string())
	}
}

#[derive(Debug)]
pub enum ExprKind {
	New(New),
	Literal(Literal),
	Range {
		start: Box<Expr>,
		inclusive: Option<bool>,
		end: Box<Expr>,
	},
	Reference(Reference),
	Intrinsic(Intrinsic),
	Call {
		callee: CalleeKind,
		arg_list: ArgList,
	},
	Unary {
		// TODO: Split to LogicalUnary, NumericUnary
		op: UnaryOperator,
		exp: Box<Expr>,
	},
	Binary {
		// TODO: Split to LogicalBinary, NumericBinary, Bit/String??
		op: BinaryOperator,
		left: Box<Expr>,
		right: Box<Expr>,
	},
	ArrayLiteral {
		type_: Option<TypeAnnotation>,
		items: Vec<Expr>,
	},
	StructLiteral {
		type_: TypeAnnotation,
		// We're using a map implementation with reliable iteration to guarantee deterministic compiler output. See discussion: https://github.com/winglang/wing/discussions/887.
		fields: IndexMap<Symbol, Expr>,
	},
	JsonMapLiteral {
		fields: IndexMap<Symbol, Expr>,
	},
	MapLiteral {
		type_: Option<TypeAnnotation>,
		fields: Vec<(Expr, Expr)>,
	},
	SetLiteral {
		type_: Option<TypeAnnotation>,
		items: Vec<Expr>,
	},
	JsonLiteral {
		is_mut: bool,
		element: Box<Expr>,
	},
	FunctionClosure(FunctionDefinition),
}

#[derive(Debug)]
pub enum CalleeKind {
	/// The callee is any expression
	Expr(Box<Expr>),
	/// The callee is a method in our super class
	SuperCall(Symbol),
}

impl Spanned for CalleeKind {
	fn span(&self) -> WingSpan {
		match self {
			CalleeKind::Expr(e) => e.span.clone(),
			CalleeKind::SuperCall(method) => method.span(),
		}
	}
}

/// File-unique identifier for each expression. This is an index of the Types.expr_types vec.
/// After type checking, each expression will have a type in that vec.
pub type ExprId = usize;

// do not derive Default, we want to be explicit about generating ids
#[derive(Debug)]
pub struct Expr {
	/// An identifier that is unique among all expressions in the AST.
	pub id: ExprId,
	/// The kind of expression.
	pub kind: ExprKind,
	/// The span of the expression.
	pub span: WingSpan,
}

impl Expr {
	pub fn new(kind: ExprKind, span: WingSpan) -> Self {
		let id = EXPR_COUNTER.fetch_add(1, Ordering::SeqCst);
		Self { id, kind, span }
	}
}

pub type ArgListId = usize;

#[derive(Debug)]
pub struct New {
	pub class: UserDefinedType,
	pub obj_id: Option<Box<Expr>>,
	pub obj_scope: Option<Box<Expr>>,
	pub arg_list: ArgList,
}

#[derive(Debug)]
pub struct ArgList {
	pub pos_args: Vec<Expr>,
	pub named_args: IndexMap<Symbol, Expr>,
	pub id: ArgListId,
	pub span: WingSpan,
}

impl ArgList {
	pub fn new(pos_args: Vec<Expr>, named_args: IndexMap<Symbol, Expr>, span: WingSpan) -> Self {
		ArgList {
			pos_args,
			named_args,
			span,
			id: ARGLIST_COUNTER.fetch_add(1, Ordering::Relaxed),
		}
	}

	pub fn new_empty(span: WingSpan) -> Self {
		Self::new(vec![], IndexMap::new(), span)
	}
}

#[derive(Debug)]
pub enum Literal {
	NonInterpolatedString(String),
	String(String),
	InterpolatedString(InterpolatedString),
	Number(f64),
	Boolean(bool),
	Nil,
}

#[derive(Debug)]
pub struct InterpolatedString {
	pub parts: Vec<InterpolatedStringPart>,
}

#[derive(Debug)]
pub enum InterpolatedStringPart {
	Static(String),
	Expr(Expr),
}

pub type ScopeId = usize;

// do not derive Default, as we want to explicitly generate IDs
#[derive(Debug)]
pub struct Scope {
	/// An identifier that is unique among all scopes in the AST.
	pub id: ScopeId,
	pub statements: Vec<Stmt>,
	pub span: WingSpan,
}

impl Scope {
	pub fn empty() -> Self {
		Self {
			id: SCOPE_COUNTER.fetch_add(1, Ordering::SeqCst),
			statements: vec![],
			span: WingSpan::default(),
		}
	}

	pub fn new(statements: Vec<Stmt>, span: WingSpan) -> Self {
		let id = SCOPE_COUNTER.fetch_add(1, Ordering::SeqCst);
		Self { id, statements, span }
	}
}

#[derive(Debug)]
pub enum UnaryOperator {
	Minus,
	Not,
	OptionalUnwrap,
}

#[derive(Debug)]
pub enum BinaryOperator {
	AddOrConcat,
	Sub,
	Mul,
	Div,
	FloorDiv,
	Mod,
	Power,
	Greater,
	GreaterOrEqual,
	Less,
	LessOrEqual,
	Equal,
	NotEqual,
	LogicalAnd,
	LogicalOr,
	UnwrapOr,
}

#[derive(Debug)]
pub enum Reference {
	/// A simple identifier: `x`
	Identifier(Symbol),
	/// A reference to a member nested inside some object `expression.x`
	InstanceMember {
		object: Box<Expr>,
		property: Symbol,
		optional_accessor: bool,
	},
	/// A reference to an accessed member of an object `expression[x]`
	///
	/// TODO: should this be a separate type of Expr? (this would require changing how `Assignment` statements are modeled)
	ElementAccess { object: Box<Expr>, index: Box<Expr> },
	/// A reference to a member inside a type: `MyType.x` or `MyEnum.A`
	TypeMember {
		type_name: UserDefinedType,
		property: Symbol,
	},
}

impl Clone for Reference {
	fn clone(&self) -> Reference {
		match self {
			Reference::Identifier(i) => Reference::Identifier(i.clone()),
			Reference::InstanceMember { .. } => panic!("Unable to clone reference to instance member"),
			Reference::TypeMember { type_name, property } => Reference::TypeMember {
				type_name: type_name.clone(),
				property: property.clone(),
			},
			Reference::ElementAccess { .. } => panic!("Unable to clone reference to element access"),
		}
	}
}

impl Spanned for Reference {
	fn span(&self) -> WingSpan {
		match self {
			Reference::Identifier(symb) => symb.span(),
			Reference::InstanceMember {
				object,
				property,
				optional_accessor: _,
			} => object.span().merge(&property.span()),
			Reference::TypeMember { type_name, property } => type_name.span().merge(&property.span()),
			Reference::ElementAccess { object, index } => {
				let mut span = object.span().merge(&index.span());
				// Add one to include the closing bracket.
				// TODO: store a dedicated span field?
				span.end.col += 1;
				span.end_offset += 1;
				span
			}
		}
	}
}

impl Display for Reference {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match &self {
			Reference::Identifier(symb) => write!(f, "{}", symb.name),
			Reference::InstanceMember {
				object,
				property,
				optional_accessor: _,
			} => {
				let obj_str = match &object.kind {
					ExprKind::Reference(r) => format!("{}", r),
					_ => "object".to_string(), // TODO!
				};
				write!(f, "{}.{}", obj_str, property.name)
			}
			Reference::TypeMember { type_name, property } => {
				write!(f, "{}.{}", type_name, property.name)
			}
			Reference::ElementAccess { .. } => {
				write!(f, "element access") // TODO!
			}
		}
	}
}

/// Represents any type that has a span.
pub trait Spanned {
	fn span(&self) -> WingSpan;
}

impl Spanned for WingSpan {
	fn span(&self) -> WingSpan {
		self.clone()
	}
}

impl Spanned for Stmt {
	fn span(&self) -> WingSpan {
		self.span.clone()
	}
}

impl Spanned for Expr {
	fn span(&self) -> WingSpan {
		self.span.clone()
	}
}

impl Spanned for Symbol {
	fn span(&self) -> WingSpan {
		self.span.clone()
	}
}

impl Spanned for TypeAnnotation {
	fn span(&self) -> WingSpan {
		self.span.clone()
	}
}

impl Spanned for UserDefinedType {
	fn span(&self) -> WingSpan {
		self.span.clone()
	}
}

impl Spanned for Scope {
	fn span(&self) -> WingSpan {
		self.span.clone()
	}
}

impl Spanned for FunctionDefinition {
	fn span(&self) -> WingSpan {
		self.span.clone()
	}
}

impl<T> Spanned for Box<T>
where
	T: Spanned,
{
	fn span(&self) -> WingSpan {
		(&**self).span()
	}
}
