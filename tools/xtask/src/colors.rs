use std::fs;
use std::io;
use std::path::Path;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};
use syn::{Expr, Ident, Item, Lit, Stmt, Token, UseTree};

const THEME: &str = "crates/flit-core/src/theme.rs";

pub(crate) fn check_dir(root: &Path, dir: &Path, problems: &mut Vec<String>) -> io::Result<()> {
    if !dir.try_exists()? {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            check_dir(root, &path, problems)?;
        } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            if relative == Path::new(THEME) {
                continue;
            }
            let violations = lines(&fs::read_to_string(&path)?)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))?;
            problems.extend(violations.iter().map(|line| format!("{}:{line}: put the color literal in {THEME}", relative.display())));
        }
    }
    Ok(())
}

pub(crate) fn lines(source: &str) -> syn::Result<Vec<usize>> {
    let mut scan = Scan::default();
    scan.visit_file(&syn::parse_file(source)?);
    scan.lines.sort_unstable();
    scan.lines.dedup();
    Ok(scan.lines)
}

#[derive(Default)]
struct Scan {
    lines: Vec<usize>,
    aliases: Vec<Ident>,
}

impl Scan {
    fn flag(&mut self, span: proc_macro2::Span) {
        self.lines.push(span.start().line);
    }

    fn visit_items(&mut self, items: &[Item]) {
        let previous = self.aliases.len();
        for item in items {
            if let Item::Use(item) = item {
                render_aliases(&item.tree, false, &mut self.aliases);
            }
        }
        for item in items {
            self.visit_item(item);
        }
        self.aliases.truncate(previous);
    }
}

impl<'ast> Visit<'ast> for Scan {
    fn visit_file(&mut self, file: &'ast syn::File) {
        self.visit_items(&file.items);
    }

    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        if let Some((_, items)) = &module.content {
            self.visit_items(items);
        }
    }
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let Expr::Path(path) = call.func.as_ref() {
            let mut parts = path.path.segments.iter().rev().map(|part| &part.ident);
            let name = parts.next();
            let parent = parts.next();
            let constructor = match (name, parent) {
                (Some(name), Some(parent)) if parent == "Color" || self.aliases.contains(parent) => name == "rgb" || name == "rgba",
                (Some(name), parent) if name == "Color" => parent.is_none_or(|p| p == "render"),
                (Some(name), None) => self.aliases.contains(name),
                _ => false,
            };
            if constructor && call.args.iter().any(literal_value) {
                self.flag(call.paren_token.span.open());
            }
        }
        visit::visit_expr_call(self, call);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        let channels = match call.method.to_string().as_str() {
            "clear_color" => 4,
            "background_pixel" | "border_pixel" | "foreground" => 1,
            _ => return visit::visit_expr_method_call(self, call),
        };
        if call.args.len() == channels && call.args.iter().all(literal_value) {
            self.flag(call.paren_token.span.open());
        }
        visit::visit_expr_method_call(self, call);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if let Ok(args) = Punctuated::<Expr, Token![,]>::parse_terminated.parse2(mac.tokens.clone()) {
            for arg in args {
                self.visit_expr(&arg);
            }
        }
    }

    fn visit_expr_lit(&mut self, expr: &'ast syn::ExprLit) {
        if let Lit::Str(text) = &expr.lit {
            let value = text.value();
            if value.strip_prefix('#').is_some_and(|hex| matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit())) {
                self.flag(text.span());
            }
        }
        visit::visit_expr_lit(self, expr);
    }

    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        let name = item.ident.to_string();
        if (name.ends_with("_COLOR") || name.ends_with("_BACKGROUND") || name == "COLOR" || name == "COLORS") && palette_literal(&item.expr) {
            self.flag(item.ident.span());
        }
        visit::visit_item_const(self, item);
    }
}

fn render_aliases(tree: &UseTree, render: bool, aliases: &mut Vec<Ident>) {
    match tree {
        UseTree::Path(path) => render_aliases(&path.tree, path.ident == "render", aliases),
        UseTree::Group(group) => {
            for member in &group.items {
                render_aliases(member, render, aliases);
            }
        }
        UseTree::Rename(alias) if render && alias.ident == "Color" => aliases.push(alias.rename.clone()),
        _ => {}
    }
}

fn literal_value(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(lit) => matches!(lit.lit, Lit::Int(_) | Lit::Float(_)),
        Expr::Binary(value) => literal_value(&value.left) && literal_value(&value.right),
        Expr::Unary(value) => literal_value(&value.expr),
        Expr::Cast(value) => literal_value(&value.expr),
        Expr::Paren(value) => literal_value(&value.expr),
        Expr::Group(value) => literal_value(&value.expr),
        Expr::Block(value) => literal_tail(&value.block),
        Expr::If(value) => literal_tail(&value.then_branch) || value.else_branch.as_ref().is_some_and(|(_, other)| literal_value(other)),
        Expr::Match(value) => value.arms.iter().any(|arm| literal_value(&arm.body)),
        _ => false,
    }
}

fn literal_tail(block: &syn::Block) -> bool {
    matches!(block.stmts.last(), Some(Stmt::Expr(expr, None)) if literal_value(expr))
}

fn palette_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Array(array) => array.elems.iter().any(palette_literal),
        Expr::Repeat(array) => palette_literal(&array.expr),
        Expr::Tuple(tuple) => tuple.elems.iter().any(palette_literal),
        _ => literal_value(expr),
    }
}
