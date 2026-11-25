//! Evaluates the literal part of TypeScript modules to JSON: what an expression is when it is made of literals and of
//! names that top-level constants and relative imports give.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    ArrayExpressionElement, BinaryOperator, BindingPattern, Declaration, ExportDefaultDeclarationKind, Expression,
    ImportDeclarationSpecifier, ModuleExportName, ObjectPropertyKind, Program, Statement,
};
use oxc_span::{GetSpan, SourceType};
use serde_json::{Map, Number, Value};

/// How deep names may refer to names before the evaluation gives up, against cycles.
const DEEPEST: usize = 48;

enum Import {
    Named(PathBuf, String),
    Namespace(PathBuf),
}

struct Module<'a> {
    source: &'a str,
    constants: HashMap<String, &'a Expression<'a>>,
    imports: HashMap<String, Import>,
    /// Exported name to the local name.
    exports: HashMap<String, String>,
    default: Option<&'a Expression<'a>>,
}

/// The modules parsed so far, by path.
pub(crate) struct Modules<'a> {
    allocator: &'a Allocator,
    modules: HashMap<PathBuf, Option<Module<'a>>>,
}

impl<'a> Modules<'a> {
    pub(crate) fn new(allocator: &'a Allocator) -> Modules<'a> {
        Modules {
            allocator,
            modules: HashMap::new(),
        }
    }

    /// What `file` exports as its default, as far as it is literal.
    pub(crate) fn default_export(&mut self, file: &Path) -> Option<Value> {
        let expression = self.module(file)?.default?;
        self.eval(file, expression, 0)
    }

    fn module(&mut self, file: &Path) -> Option<&Module<'a>> {
        if !self.modules.contains_key(file) {
            let parsed = self.parse(file);
            self.modules.insert(file.to_path_buf(), parsed);
        }
        self.modules.get(file)?.as_ref()
    }

    fn parse(&self, file: &Path) -> Option<Module<'a>> {
        let text = std::fs::read_to_string(file).ok()?;
        let source: &'a str = self.allocator.alloc_str(&text);
        let parsed = oxc_parser::Parser::new(self.allocator, source, SourceType::ts()).parse();
        if parsed.fatal_error {
            return None;
        }
        let program: &'a Program<'a> = self.allocator.alloc(parsed.program);
        let dir = file.parent().unwrap_or(Path::new("."));
        let mut module = Module {
            source,
            constants: HashMap::new(),
            imports: HashMap::new(),
            exports: HashMap::new(),
            default: None,
        };
        for statement in &program.body {
            match statement {
                Statement::VariableDeclaration(declaration) => constants(&declaration.declarations, &mut module),
                Statement::ExportDeclaration(export) => {
                    if let Declaration::VariableDeclaration(declaration) = &export.declaration {
                        for declarator in &declaration.declarations {
                            if let Some(name) = binding_name(&declarator.id) {
                                module.exports.insert(name.clone(), name);
                            }
                        }
                        constants(&declaration.declarations, &mut module);
                    }
                }
                Statement::ExportNamedDeclaration(export) => {
                    for specifier in &export.specifiers {
                        module
                            .exports
                            .insert(export_name(&specifier.exported), export_name(&specifier.local));
                    }
                }
                Statement::ExportDefaultDeclaration(export) => {
                    if let Some(expression) = default_expression(&export.declaration) {
                        module.default = Some(expression);
                    }
                }
                Statement::ImportDeclaration(import) => {
                    let Some(path) = relative(dir, import.source.value.as_str().unwrap_or_default()) else {
                        continue;
                    };
                    for specifier in import.specifiers.iter().flatten() {
                        let (local, what) = match specifier {
                            ImportDeclarationSpecifier::ImportSpecifier(one) => (
                                one.local.name.to_string(),
                                Import::Named(path.clone(), export_name(&one.imported)),
                            ),
                            ImportDeclarationSpecifier::ImportDefaultSpecifier(one) => (
                                one.local.name.to_string(),
                                Import::Named(path.clone(), "default".to_string()),
                            ),
                            ImportDeclarationSpecifier::ImportNamespaceSpecifier(one) => {
                                (one.local.name.to_string(), Import::Namespace(path.clone()))
                            }
                        };
                        module.imports.insert(local, what);
                    }
                }
                _ => {}
            }
        }
        Some(module)
    }

    /// What `name`, exported by `file`, is.
    fn exported(&mut self, file: &Path, name: &str, depth: usize) -> Option<Value> {
        let module = self.module(file)?;
        if name == "default" {
            let expression = module.default?;
            return self.eval(file, expression, depth + 1);
        }
        let local = module.exports.get(name).cloned().unwrap_or_else(|| name.to_string());
        self.name(file, &local, depth + 1)
    }

    /// What the name `name` in `file` is.
    fn name(&mut self, file: &Path, name: &str, depth: usize) -> Option<Value> {
        if depth > DEEPEST {
            return None;
        }
        let module = self.module(file)?;
        if let Some(expression) = module.constants.get(name).copied() {
            return self.eval(file, expression, depth + 1);
        }
        match module.imports.get(name)? {
            Import::Named(path, exported) => {
                let (path, exported) = (path.clone(), exported.clone());
                self.exported(&path, &exported, depth + 1)
            }
            Import::Namespace(_) => None,
        }
    }

    fn source_of(&mut self, file: &Path, expression: &Expression<'a>) -> String {
        let span = expression.span();
        self.module(file)
            .and_then(|module| module.source.get(span.start as usize..span.end as usize))
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// What `expression` in `file` is, as far as it is literal.
    fn eval(&mut self, file: &Path, expression: &'a Expression<'a>, depth: usize) -> Option<Value> {
        if depth > DEEPEST {
            return None;
        }
        match expression {
            Expression::StringLiteral(s) => Some(Value::String(s.value.as_str()?.to_string())),
            Expression::NumericLiteral(n) => Number::from_f64(n.value).map(Value::Number),
            Expression::BooleanLiteral(b) => Some(Value::Bool(b.value)),
            Expression::TemplateLiteral(t) => {
                let mut text = String::new();
                for (at, quasi) in t.quasis.iter().enumerate() {
                    text.push_str(quasi.value.cooked?.as_str()?);
                    if let Some(inner) = t.expressions.get(at) {
                        match self.eval(file, inner, depth + 1)? {
                            Value::String(s) => text.push_str(&s),
                            Value::Number(n) => text.push_str(&n.to_string()),
                            _ => return None,
                        }
                    }
                }
                Some(Value::String(text))
            }
            Expression::BinaryExpression(b) if b.operator == BinaryOperator::Addition => {
                match (
                    self.eval(file, &b.left, depth + 1)?,
                    self.eval(file, &b.right, depth + 1)?,
                ) {
                    (Value::String(a), Value::String(b)) => Some(Value::String(a + &b)),
                    _ => None,
                }
            }
            Expression::Identifier(id) => self.name(file, id.name.as_str(), depth + 1),
            Expression::ParenthesizedExpression(p) => self.eval(file, &p.expression, depth + 1),
            Expression::TSAsExpression(e) => self.eval(file, &e.expression, depth + 1),
            Expression::TSSatisfiesExpression(e) => self.eval(file, &e.expression, depth + 1),
            Expression::TSNonNullExpression(e) => self.eval(file, &e.expression, depth + 1),
            Expression::StaticMemberExpression(member) => {
                let property = member.property.name.as_str();
                if let Expression::Identifier(id) = &member.object
                    && let Some(Import::Namespace(path)) = self
                        .module(file)
                        .and_then(|module| module.imports.get(id.name.as_str()))
                {
                    let path = path.clone();
                    return self.exported(&path, property, depth + 1);
                }
                self.eval(file, &member.object, depth + 1)?.get(property).cloned()
            }
            Expression::ArrayExpression(array) => {
                let mut out = Vec::new();
                for element in &array.elements {
                    match element {
                        ArrayExpressionElement::SpreadElement(spread) => {
                            if let Some(Value::Array(more)) = self.eval(file, &spread.argument, depth + 1) {
                                out.extend(more);
                            }
                        }
                        ArrayExpressionElement::Elision(_) => {}
                        other => {
                            if let Some(value) = self.eval(file, other.to_expression(), depth + 1) {
                                out.push(value);
                            }
                        }
                    }
                }
                Some(Value::Array(out))
            }
            Expression::ObjectExpression(object) => Some(Value::Object(self.object(file, object, depth)?)),
            Expression::CallExpression(call) => self.call(file, &call.callee, depth),
            _ => None,
        }
    }

    /// A call of an arrow function of this module that returns an expression, such as cargo's spec, which takes the
    /// toolchain as an argument: its body, the parameters left unknown.
    fn call(&mut self, file: &Path, callee: &'a Expression<'a>, depth: usize) -> Option<Value> {
        let Expression::Identifier(id) = callee else {
            return None;
        };
        let function = self.module(file)?.constants.get(id.name.as_str()).copied()?;
        let Expression::ArrowFunctionExpression(arrow) = function else {
            return None;
        };
        self.eval(file, arrow.get_expression()?, depth + 1)
    }

    fn object(
        &mut self,
        file: &Path,
        object: &'a oxc_ast::ast::ObjectExpression<'a>,
        depth: usize,
    ) -> Option<Map<String, Value>> {
        let mut out = Map::new();
        for property in &object.properties {
            match property {
                ObjectPropertyKind::SpreadProperty(spread) => {
                    if let Some(Value::Object(more)) = self.eval(file, &spread.argument, depth + 1) {
                        out.extend(more);
                    }
                }
                ObjectPropertyKind::ObjectProperty(property) => {
                    if property.method {
                        continue;
                    }
                    let Some(key) = property.key.literal_name().and_then(|n| n.as_str().map(str::to_string)) else {
                        continue;
                    };
                    let value = if key == "generators" {
                        Some(self.generators(file, &property.value, depth))
                    } else {
                        self.eval(file, &property.value, depth + 1)
                    };
                    if let Some(value) = value {
                        out.insert(key, value);
                    }
                }
            }
        }
        Some(out)
    }

    /// A generator or a list of them: what is literal in each, and under `dynamic` the source of the generator or
    /// its script, which the dynamic sources recognise.
    fn generators(&mut self, file: &Path, expression: &'a Expression<'a>, depth: usize) -> Value {
        if let Expression::ArrayExpression(array) = expression {
            let all = array
                .elements
                .iter()
                .filter(|element| element.is_expression())
                .map(|element| self.generator(file, element.to_expression(), depth))
                .collect();
            return Value::Array(all);
        }
        self.generator(file, expression, depth)
    }

    fn generator(&mut self, file: &Path, expression: &'a Expression<'a>, depth: usize) -> Value {
        let mut value = match self.eval(file, expression, depth + 1) {
            Some(Value::Object(object)) => object,
            _ => Map::new(),
        };
        let dynamic = match (expression, value.get("script")) {
            (_, Some(Value::Array(script))) => Some(format!(
                "script:{}",
                script.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")
            )),
            (_, Some(Value::String(script))) => Some(format!("script:{script}")),
            (Expression::ObjectExpression(_), _) => None,
            _ => Some(self.source_of(file, expression)),
        };
        if let Some(dynamic) = dynamic.filter(|d| d.len() < 120) {
            value.insert("dynamic".to_string(), Value::String(dynamic));
        }
        Value::Object(value)
    }
}

fn constants<'a>(declarators: &'a [oxc_ast::ast::VariableDeclarator<'a>], module: &mut Module<'a>) {
    for declarator in declarators {
        if let (Some(name), Some(init)) = (binding_name(&declarator.id), &declarator.init) {
            module.constants.insert(name, init);
        }
    }
}

fn binding_name(pattern: &BindingPattern<'_>) -> Option<String> {
    pattern.get_identifier_name().map(|name| name.to_string())
}

fn export_name(name: &ModuleExportName<'_>) -> String {
    match name {
        ModuleExportName::IdentifierName(id) => id.name.to_string(),
        ModuleExportName::IdentifierReference(id) => id.name.to_string(),
        ModuleExportName::StringLiteral(s) => s.value.as_str().unwrap_or_default().to_string(),
    }
}

fn default_expression<'a>(kind: &'a ExportDefaultDeclarationKind<'a>) -> Option<&'a Expression<'a>> {
    kind.as_expression()
}

/// The file a relative import names; none for a package's.
fn relative(dir: &Path, source: &str) -> Option<PathBuf> {
    if !source.starts_with('.') {
        return None;
    }
    let base = dir.join(source);
    [base.with_extension("ts"), base.join("index.ts"), base.clone()]
        .into_iter()
        .find(|candidate| candidate.is_file())
}
