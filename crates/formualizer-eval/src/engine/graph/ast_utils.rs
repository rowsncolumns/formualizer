use super::*;
use formualizer_parse::parser::{ASTNode, ASTNodeType, ReferenceType};

pub(super) fn update_sheet_references_in_ast(
    ast: &ASTNode,
    old_name: &str,
    new_name: &str,
) -> ASTNode {
    match &ast.node_type {
        ASTNodeType::Reference { reference, .. } => {
            let updated_ref = match reference {
                ReferenceType::Cell {
                    sheet,
                    row,
                    col,
                    row_abs,
                    col_abs,
                } => {
                    if sheet.as_deref() == Some(old_name) {
                        ReferenceType::Cell {
                            sheet: Some(new_name.to_string()),
                            row: *row,
                            col: *col,
                            row_abs: *row_abs,
                            col_abs: *col_abs,
                        }
                    } else {
                        reference.clone()
                    }
                }
                ReferenceType::Range {
                    sheet,
                    start_row,
                    start_col,
                    end_row,
                    end_col,
                    start_row_abs,
                    start_col_abs,
                    end_row_abs,
                    end_col_abs,
                } => {
                    if sheet.as_deref() == Some(old_name) {
                        ReferenceType::Range {
                            sheet: Some(new_name.to_string()),
                            start_row: *start_row,
                            start_col: *start_col,
                            end_row: *end_row,
                            end_col: *end_col,
                            start_row_abs: *start_row_abs,
                            start_col_abs: *start_col_abs,
                            end_row_abs: *end_row_abs,
                            end_col_abs: *end_col_abs,
                        }
                    } else {
                        reference.clone()
                    }
                }
                _ => reference.clone(),
            };

            ASTNode {
                node_type: ASTNodeType::Reference {
                    original: String::new(),
                    reference: updated_ref,
                },
                source_token: None,
                contains_volatile: ast.contains_volatile,
            }
        }
        // A chain is as deep as it is long: walk its left spine with a loop (FX-02).
        ASTNodeType::BinaryOp { .. } => ast.fold_binary_chain(
            |operand| update_sheet_references_in_ast(operand, old_name, new_name),
            rebuilt_binary,
        ),
        ASTNodeType::UnaryOp { op, expr } => ASTNode {
            node_type: ASTNodeType::UnaryOp {
                op: op.clone(),
                expr: Box::new(update_sheet_references_in_ast(expr, old_name, new_name)),
            },
            source_token: None,
            contains_volatile: ast.contains_volatile,
        },
        ASTNodeType::Function { name, args } => ASTNode {
            node_type: ASTNodeType::Function {
                name: name.clone(),
                args: args
                    .iter()
                    .map(|arg| update_sheet_references_in_ast(arg, old_name, new_name))
                    .collect(),
            },
            source_token: None,
            contains_volatile: ast.contains_volatile,
        },
        ASTNodeType::Array(rows) => ASTNode {
            node_type: ASTNodeType::Array(
                rows.iter()
                    .map(|row| {
                        row.iter()
                            .map(|cell| update_sheet_references_in_ast(cell, old_name, new_name))
                            .collect()
                    })
                    .collect(),
            ),
            source_token: None,
            contains_volatile: ast.contains_volatile,
        },
        _ => ast.clone(),
    }
}

pub(super) fn update_internal_sheet_references(
    ast: &ASTNode,
    source_name: &str,
    new_name: &str,
    source_id: SheetId,
    new_id: SheetId,
) -> ASTNode {
    let _ = (source_id, new_id);

    match &ast.node_type {
        ASTNodeType::Reference { reference, .. } => {
            let updated_ref = match reference {
                ReferenceType::Cell {
                    sheet,
                    row,
                    col,
                    row_abs,
                    col_abs,
                } => {
                    // Update references without sheet name (internal) or with source sheet name.
                    if sheet.is_none() || sheet.as_deref() == Some(source_name) {
                        ReferenceType::Cell {
                            sheet: Some(new_name.to_string()),
                            row: *row,
                            col: *col,
                            row_abs: *row_abs,
                            col_abs: *col_abs,
                        }
                    } else {
                        reference.clone()
                    }
                }
                ReferenceType::Range {
                    sheet,
                    start_row,
                    start_col,
                    end_row,
                    end_col,
                    start_row_abs,
                    start_col_abs,
                    end_row_abs,
                    end_col_abs,
                } => {
                    if sheet.is_none() || sheet.as_deref() == Some(source_name) {
                        ReferenceType::Range {
                            sheet: Some(new_name.to_string()),
                            start_row: *start_row,
                            start_col: *start_col,
                            end_row: *end_row,
                            end_col: *end_col,
                            start_row_abs: *start_row_abs,
                            start_col_abs: *start_col_abs,
                            end_row_abs: *end_row_abs,
                            end_col_abs: *end_col_abs,
                        }
                    } else {
                        reference.clone()
                    }
                }
                _ => reference.clone(),
            };

            ASTNode {
                node_type: ASTNodeType::Reference {
                    original: String::new(),
                    reference: updated_ref,
                },
                source_token: None,
                contains_volatile: ast.contains_volatile,
            }
        }
        ASTNodeType::BinaryOp { .. } => ast.fold_binary_chain(
            |operand| {
                update_internal_sheet_references(operand, source_name, new_name, source_id, new_id)
            },
            rebuilt_binary,
        ),
        ASTNodeType::UnaryOp { op, expr } => ASTNode {
            node_type: ASTNodeType::UnaryOp {
                op: op.clone(),
                expr: Box::new(update_internal_sheet_references(
                    expr,
                    source_name,
                    new_name,
                    source_id,
                    new_id,
                )),
            },
            source_token: None,
            contains_volatile: ast.contains_volatile,
        },
        ASTNodeType::Function { name, args } => ASTNode {
            node_type: ASTNodeType::Function {
                name: name.clone(),
                args: args
                    .iter()
                    .map(|arg| {
                        update_internal_sheet_references(
                            arg,
                            source_name,
                            new_name,
                            source_id,
                            new_id,
                        )
                    })
                    .collect(),
            },
            source_token: None,
            contains_volatile: ast.contains_volatile,
        },
        ASTNodeType::Array(rows) => ASTNode {
            node_type: ASTNodeType::Array(
                rows.iter()
                    .map(|row| {
                        row.iter()
                            .map(|cell| {
                                update_internal_sheet_references(
                                    cell,
                                    source_name,
                                    new_name,
                                    source_id,
                                    new_id,
                                )
                            })
                            .collect()
                    })
                    .collect(),
            ),
            source_token: None,
            contains_volatile: ast.contains_volatile,
        },
        _ => ast.clone(),
    }
}

/// A binary node rebuilt over new operands, as the sheet-reference rewrites above build it (no
/// source token).
fn rebuilt_binary(node: &ASTNode, left: ASTNode, right: ASTNode) -> ASTNode {
    let ASTNodeType::BinaryOp { op, .. } = &node.node_type else {
        unreachable!("fold_binary_chain combines binary nodes only");
    };
    ASTNode {
        node_type: ASTNodeType::BinaryOp {
            op: op.clone(),
            left: Box::new(left),
            right: Box::new(right),
        },
        source_token: None,
        contains_volatile: node.contains_volatile,
    }
}
