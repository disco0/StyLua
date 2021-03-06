use full_moon::ast::{
    punctuated::{Pair, Punctuated},
    span::ContainedSpan,
    Call, Expression, FunctionArgs, FunctionBody, FunctionCall, FunctionDeclaration, FunctionName,
    LocalFunction, MethodCall, Parameter, Value,
};
use full_moon::tokenizer::{Symbol, Token, TokenKind, TokenReference, TokenType};
use std::borrow::Cow;
use std::boxed::Box;

use crate::formatters::{
    get_line_ending_character,
    trivia_formatter::{self, FormatTriviaType},
    trivia_util, CodeFormatter,
};

impl CodeFormatter {
    /// Formats an Anonymous Function
    /// This doesn't have its own struct, but it is part of Value::Function
    pub fn format_anonymous_function<'ast>(
        &mut self,
        function_token: &TokenReference<'ast>,
        function_body: &FunctionBody<'ast>,
    ) -> (Cow<'ast, TokenReference<'ast>>, FunctionBody<'ast>) {
        let function_token_range = CodeFormatter::get_token_range(function_token.token());
        let additional_indent_level = self.get_range_indent_increase(function_token_range); //code_formatter.get_token_indent_increase(function_token.token());

        let function_token = crate::fmt_symbol!(self, function_token, "function");
        let mut function_body = self.format_function_body(function_body);

        // Need to insert any additional trivia, as it isn't being inserted elsewhere
        #[cfg(feature = "luau")]
        {
            let (parameters_parentheses, return_type) = match function_body.return_type() {
                Some(return_type) => (
                    function_body.parameters_parentheses().to_owned(),
                    Some(trivia_formatter::type_specifier_add_trailing_trivia(
                        return_type.to_owned(),
                        FormatTriviaType::Append(vec![self.create_newline_trivia()]),
                    )),
                ),
                None => {
                    // No return type, so add trivia to the parentheses instead
                    let parameters_parentheses = trivia_formatter::contained_span_add_trivia(
                        function_body.parameters_parentheses().to_owned(),
                        FormatTriviaType::NoChange,
                        FormatTriviaType::Append(vec![self.create_newline_trivia()]),
                    );
                    (parameters_parentheses, None)
                }
            };

            function_body = function_body
                .with_parameters_parentheses(parameters_parentheses)
                .with_return_type(return_type);
        }

        #[cfg(not(feature = "luau"))]
        {
            let parameters_parentheses = trivia_formatter::contained_span_add_trivia(
                function_body.parameters_parentheses().to_owned(),
                FormatTriviaType::NoChange,
                FormatTriviaType::Append(vec![self.create_newline_trivia()]),
            );
            function_body = function_body.with_parameters_parentheses(parameters_parentheses);
        };

        let end_token = Cow::Owned(trivia_formatter::token_reference_add_trivia(
            function_body.end_token().to_owned(),
            FormatTriviaType::Append(vec![self.create_indent_trivia(additional_indent_level)]),
            FormatTriviaType::NoChange,
        ));

        (function_token, function_body.with_end_token(end_token))
    }

    /// Formats a Call node
    pub fn format_call<'ast>(&mut self, call: &Call<'ast>) -> Call<'ast> {
        match call {
            Call::AnonymousCall(function_args) => {
                Call::AnonymousCall(self.format_function_args(function_args))
            }
            Call::MethodCall(method_call) => Call::MethodCall(self.format_method_call(method_call)),
        }
    }

    /// Formats a FunctionArgs node
    pub fn format_function_args<'ast>(
        &mut self,
        function_args: &FunctionArgs<'ast>,
    ) -> FunctionArgs<'ast> {
        match function_args {
            FunctionArgs::Parentheses {
                parentheses,
                arguments,
            } => {
                let (start_parens, end_parens) = parentheses.tokens();
                // Find the range of the function arguments
                let function_call_range = (
                    start_parens.end_position().bytes(),
                    end_parens.start_position().bytes(),
                );
                let mut is_multiline = (function_call_range.1 - function_call_range.0) > 80; // TODO: Properly determine this arbitrary number, and see if other factors should come into play
                let current_arguments = arguments.pairs();

                // Format all the arguments, so that we can prepare them and check to see whether they need expanding
                // We will ignore punctuation for now
                let mut formatted_arguments = Vec::new();
                for argument in arguments.iter() {
                    formatted_arguments.push(self.format_expression(argument))
                }

                // Apply some heuristics to determine whether we should expand the function call
                // TODO: These are subject to change

                // If there is a comment present anywhere in between the start parentheses and end parentheses, we should keep it multiline
                let force_mutliline: bool =
                    if trivia_util::token_trivia_contains_comments(start_parens.trailing_trivia())
                        || trivia_util::token_trivia_contains_comments(end_parens.leading_trivia())
                    {
                        true
                    } else {
                        let mut contains_comments = false;
                        for argument in arguments.pairs() {
                            if trivia_util::expression_contains_comments(argument.value()) {
                                contains_comments = true;
                            } else if let Some(punctuation) = argument.punctuation() {
                                if trivia_util::token_contains_comments(punctuation) {
                                    contains_comments = true;
                                }
                            };

                            if contains_comments {
                                break;
                            }
                        }

                        contains_comments
                    };

                if force_mutliline {
                    is_multiline = true;
                }

                if is_multiline && !force_mutliline {
                    // If we only have one argument then we will not make it multi line (expanding it would have little value)
                    if formatted_arguments.len() == 1 {
                        is_multiline = false;
                    } else {
                        // Find how far we are currently indented, we can use this to determine when to expand
                        // TODO: We need to add more to this - there may be quite a lot before this function call
                        // TODO: include additional_indent_level
                        let mut width_passed = self.get_indent_width();
                        let mut keep_single_line = false;
                        // Check to see if we have a table constructor, or anonymous function
                        for argument in formatted_arguments.iter() {
                            // TODO: Do we need to worry about parentheses or UnOp?
                            if let Expression::Value { value, .. } = argument {
                                match &**value {
                                    Value::Function(_) => {
                                        // An anonymous function should always be expanded
                                        // This is safe to prevent expansion
                                        keep_single_line = true;
                                    }
                                    Value::TableConstructor(table) => {
                                        // Check to see whether it has been expanded
                                        let start_brace = table.braces().tokens().0;
                                        let is_expanded = trivia_util::trivia_contains_newline(
                                            start_brace.trailing_trivia(),
                                        );
                                        if is_expanded {
                                            keep_single_line = true
                                        } else {
                                            // We have a collapsed table constructor - add the width, and if it fails,
                                            // we need to expand
                                            width_passed += argument.to_string().len();
                                            if width_passed > 80 {
                                                break;
                                            }
                                        }
                                    }
                                    _ => {
                                        // If we previously had a table/anonymous function, and we have something else
                                        // in the mix, we should not expand
                                        if keep_single_line {
                                            keep_single_line = false;
                                            break;
                                        }
                                        width_passed += argument.to_string().len();
                                        if width_passed > 80 {
                                            // We have passed 80 characters without a table or anonymous function
                                            // There is nothing else stopping us from expanding - so we will
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        if keep_single_line {
                            is_multiline = false;
                        }
                    }
                }

                if is_multiline {
                    // TODO: This is similar to multiline in TableConstructor, can we resolve?
                    // Format start and end brace properly with correct trivia

                    // Calculate to see if the end parentheses requires any additional indentation
                    let end_parens_additional_indent_level = self.get_range_indent_increase((
                        end_parens.start_position().bytes(),
                        end_parens.end_position().bytes(),
                    ));
                    let end_parens_leading_trivia =
                        vec![self.create_indent_trivia(end_parens_additional_indent_level)];

                    // Add new_line trivia to start_parens
                    let start_parens_token = crate::fmt_symbol!(self, start_parens, "(");
                    let start_parens_token = trivia_formatter::token_reference_add_trivia(
                        start_parens_token.into_owned(),
                        FormatTriviaType::NoChange,
                        FormatTriviaType::Append(vec![self.create_newline_trivia()]),
                    );

                    let end_parens_token = TokenReference::new(
                        end_parens_leading_trivia,
                        Token::new(TokenType::Symbol {
                            symbol: Symbol::RightParen,
                        }),
                        vec![],
                    );

                    let parentheses = ContainedSpan::new(
                        Cow::Owned(start_parens_token),
                        self.format_symbol(end_parens, &end_parens_token),
                    );

                    let mut formatted_arguments = Punctuated::new();

                    self.add_indent_range(function_call_range);

                    for argument in current_arguments {
                        let argument_range =
                            CodeFormatter::get_range_in_expression(argument.value());
                        let additional_indent_level =
                            self.get_range_indent_increase(argument_range);

                        // Unfortunately, we need to format again, taking into account in indent increase
                        // TODO: Can we fix this? We don't want to have to format twice
                        let formatted_argument = trivia_formatter::expression_add_leading_trivia(
                            self.format_expression(argument.value()),
                            FormatTriviaType::Append(vec![
                                self.create_indent_trivia(additional_indent_level)
                            ]),
                        );

                        let punctuation = match argument.punctuation() {
                            Some(punctuation) => {
                                // Continue adding a comma and a new line for multiline function args
                                let symbol = crate::fmt_symbol!(self, punctuation, ",");
                                let symbol = trivia_formatter::token_reference_add_trivia(
                                    symbol.into_owned(),
                                    FormatTriviaType::NoChange,
                                    FormatTriviaType::Append(vec![self.create_newline_trivia()]),
                                );

                                Some(Cow::Owned(symbol))
                            }
                            None => Some(Cow::Owned(TokenReference::new(
                                vec![],
                                Token::new(TokenType::Whitespace {
                                    characters: Cow::Owned(get_line_ending_character(
                                        &self.config.line_endings,
                                    )),
                                }),
                                vec![],
                            ))),
                        };

                        formatted_arguments.push(Pair::new(formatted_argument, punctuation))
                    }

                    FunctionArgs::Parentheses {
                        parentheses,
                        arguments: formatted_arguments,
                    }
                } else {
                    let parentheses = self.format_contained_span(&parentheses);

                    // If theres comments connected to the opening parentheses, we need to move them
                    let (start_parens, end_parens) = parentheses.tokens();
                    let mut parens_comments: Vec<Token<'ast>> = start_parens
                        .trailing_trivia()
                        .filter(|token| {
                            token.token_kind() == TokenKind::SingleLineComment
                                || token.token_kind() == TokenKind::MultiLineComment
                        })
                        .map(|x| {
                            // Prepend a single space beforehand
                            vec![Token::new(TokenType::spaces(1)), x.to_owned()]
                        })
                        .flatten()
                        .collect();

                    // Format the arguments, and move any comments within them
                    let (formatted_arguments, mut comments_buffer) =
                        self.format_punctuated(arguments, &CodeFormatter::format_expression);

                    parens_comments.append(&mut comments_buffer);

                    // Recreate parentheses with the comments removed from the opening parens
                    // and all the comments placed at the end of the closing parens
                    let parentheses = ContainedSpan::new(
                        Cow::Owned(trivia_formatter::token_reference_add_trivia(
                            start_parens.to_owned(),
                            FormatTriviaType::NoChange,
                            FormatTriviaType::Replace(vec![]),
                        )),
                        Cow::Owned(trivia_formatter::token_reference_add_trivia(
                            end_parens.to_owned(),
                            FormatTriviaType::NoChange,
                            FormatTriviaType::Append(parens_comments),
                        )),
                    );

                    FunctionArgs::Parentheses {
                        parentheses,
                        arguments: formatted_arguments,
                    }
                }
            }

            FunctionArgs::String(token_reference) => {
                let mut arguments = Punctuated::new();
                let new_expression = self.format_expression(&Expression::Value {
                    value: Box::new(Value::String(token_reference.to_owned())),
                    binop: None,
                    #[cfg(feature = "luau")]
                    as_assertion: None,
                });

                // Remove any trailing comments from the expression, and move them into a buffer
                let (new_expression, comments_buffer) =
                    trivia_util::get_expression_trailing_comments(&new_expression);

                // Create parentheses, and add the trailing comments to the end of the parentheses
                let parentheses = trivia_formatter::contained_span_add_trivia(
                    ContainedSpan::new(
                        Cow::Owned(TokenReference::symbol("(").unwrap()),
                        Cow::Owned(TokenReference::symbol(")").unwrap()),
                    ),
                    FormatTriviaType::NoChange,
                    FormatTriviaType::Append(comments_buffer),
                );

                arguments.push(Pair::new(new_expression, None)); // Only single argument, so no trailing comma

                FunctionArgs::Parentheses {
                    parentheses,
                    arguments,
                }
            }

            FunctionArgs::TableConstructor(table_constructor) => {
                let mut arguments = Punctuated::new();
                let new_expression = self.format_expression(&Expression::Value {
                    value: Box::new(Value::TableConstructor(table_constructor.to_owned())),
                    binop: None,
                    #[cfg(feature = "luau")]
                    as_assertion: None,
                });

                // Remove any trailing comments from the expression, and move them into a buffer
                let (new_expression, comments_buffer) =
                    trivia_util::get_expression_trailing_comments(&new_expression);

                // Create parentheses, and add the trailing comments to the end of the parentheses
                let parentheses = trivia_formatter::contained_span_add_trivia(
                    ContainedSpan::new(
                        Cow::Owned(TokenReference::symbol("(").unwrap()),
                        Cow::Owned(TokenReference::symbol(")").unwrap()),
                    ),
                    FormatTriviaType::NoChange,
                    FormatTriviaType::Append(comments_buffer),
                );

                arguments.push(Pair::new(new_expression, None)); // Only single argument, so no trailing comma

                FunctionArgs::Parentheses {
                    parentheses,
                    arguments,
                }
            }
        }
    }

    /// Formats a FunctionBody node
    pub fn format_function_body<'ast>(
        &mut self,
        function_body: &FunctionBody<'ast>,
    ) -> FunctionBody<'ast> {
        let parameters_parentheses =
            self.format_contained_span(function_body.parameters_parentheses());
        let formatted_parameters = self.format_parameters(function_body);

        #[cfg(feature = "luau")]
        let mut type_specifiers;
        #[cfg(feature = "luau")]
        let return_type;

        #[cfg(feature = "luau")]
        {
            type_specifiers = Vec::new();
            for specifier in function_body.type_specifiers() {
                let formatted_specifier = match specifier {
                    Some(specifier) => Some(self.format_type_specifier(specifier)),
                    None => None,
                };
                type_specifiers.push(formatted_specifier);
            }

            return_type = match function_body.return_type() {
                Some(return_type) => Some(self.format_type_specifier(return_type)),
                None => None,
            };
        }

        let end_token = self.format_end_token(function_body.end_token());

        let function_body = function_body
            .to_owned()
            .with_parameters_parentheses(parameters_parentheses)
            .with_parameters(formatted_parameters)
            .with_end_token(end_token);

        #[cfg(feature = "luau")]
        let function_body = function_body
            .with_type_specifiers(type_specifiers)
            .with_return_type(return_type);

        function_body
    }

    /// Formats a FunctionCall node
    pub fn format_function_call<'ast>(
        &mut self,
        function_call: &FunctionCall<'ast>,
    ) -> FunctionCall<'ast> {
        let formatted_prefix = self.format_prefix(function_call.prefix());
        let formatted_suffixes = function_call
            .iter_suffixes()
            .map(|x| self.format_suffix(x))
            .collect();

        FunctionCall::new(formatted_prefix).with_suffixes(formatted_suffixes)
    }

    /// Formats a FunctionName node
    pub fn format_function_name<'ast>(
        &mut self,
        function_name: &FunctionName<'ast>,
    ) -> FunctionName<'ast> {
        // TODO: This is based off formatters::format_punctuated - can we merge them into one?
        let mut formatted_names = Punctuated::new();
        for pair in function_name.names().to_owned().into_pairs() {
            // Format Punctuation
            match pair {
                Pair::Punctuated(value, punctuation) => {
                    let formatted_punctuation = crate::fmt_symbol!(self, &punctuation, ".");
                    let formatted_value = self.format_token_reference(&value);
                    formatted_names.push(Pair::new(formatted_value, Some(formatted_punctuation)));
                }
                Pair::End(value) => {
                    let formatted_value = self.format_token_reference(&value);
                    formatted_names.push(Pair::new(formatted_value, None));
                }
            }
        }

        let mut formatted_method: Option<(
            Cow<'ast, TokenReference<'ast>>,
            Cow<'ast, TokenReference<'ast>>,
        )> = None;

        if let Some(method_colon) = function_name.method_colon() {
            if let Some(token_reference) = function_name.method_name() {
                formatted_method = Some((
                    crate::fmt_symbol!(self, method_colon, ":"),
                    Cow::Owned(self.format_plain_token_reference(token_reference)),
                ));
            }
        };

        FunctionName::new(formatted_names).with_method(formatted_method)
    }

    /// Formats a FunctionDeclaration node
    pub fn format_function_declaration<'ast>(
        &mut self,
        function_declaration: &FunctionDeclaration<'ast>,
    ) -> FunctionDeclaration<'ast> {
        let function_token =
            crate::fmt_symbol!(self, function_declaration.function_token(), "function ");
        let formatted_function_name = self.format_function_name(function_declaration.name());
        let formatted_function_body = self.format_function_body(function_declaration.body());

        FunctionDeclaration::new(formatted_function_name)
            .with_function_token(function_token)
            .with_body(formatted_function_body)
    }

    /// Formats a LocalFunction node
    pub fn format_local_function<'ast>(
        &mut self,
        local_function: &LocalFunction<'ast>,
    ) -> LocalFunction<'ast> {
        let local_token = crate::fmt_symbol!(self, local_function.local_token(), "local ");
        let function_token = crate::fmt_symbol!(self, local_function.function_token(), "function ");
        let formatted_name = Cow::Owned(self.format_plain_token_reference(local_function.name()));
        let formatted_function_body = self.format_function_body(local_function.func_body());

        LocalFunction::new(formatted_name)
            .with_local_token(local_token)
            .with_function_token(function_token)
            .with_func_body(formatted_function_body)
    }

    /// Formats a MethodCall node
    pub fn format_method_call<'ast>(&mut self, method_call: &MethodCall<'ast>) -> MethodCall<'ast> {
        let formatted_colon_token = self.format_plain_token_reference(method_call.colon_token());
        let formatted_name = self.format_plain_token_reference(method_call.name());
        let formatted_function_args = self.format_function_args(method_call.args());

        MethodCall::new(Cow::Owned(formatted_name), formatted_function_args)
            .with_colon_token(Cow::Owned(formatted_colon_token))
    }

    /// Formats a single Parameter node
    pub fn format_parameter<'ast>(&mut self, parameter: &Parameter<'ast>) -> Parameter<'ast> {
        match parameter {
            Parameter::Ellipse(token) => Parameter::Ellipse(crate::fmt_symbol!(self, token, "...")),
            Parameter::Name(token_reference) => {
                Parameter::Name(self.format_token_reference(token_reference))
            }
        }
    }

    /// Utilises the FunctionBody iterator to format a list of Parameter nodes
    fn format_parameters<'ast>(
        &mut self,
        function_body: &FunctionBody<'ast>,
    ) -> Punctuated<'ast, Parameter<'ast>> {
        let mut formatted_parameters = Punctuated::new();
        let mut parameters_iterator = function_body.parameters().iter().peekable();
        while let Some(parameter) = parameters_iterator.next() {
            let formatted_parameter = self.format_parameter(parameter);
            let mut punctuation = None;

            if parameters_iterator.peek().is_some() {
                punctuation = Some(Cow::Owned(TokenReference::symbol(", ").unwrap()));
            }

            formatted_parameters.push(Pair::new(formatted_parameter, punctuation));
        }
        formatted_parameters
    }
}
