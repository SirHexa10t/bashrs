/// All user-facing commands.
/// Add a variant here to register a new command everywhere automatically.
pub enum Command {
    Hello,
    Histfmt,
}

impl Command {
    /// All variants in order — used by generate_sourceable.
    pub const ALL: &'static [Command] = &[
        Command::Hello,
        Command::Histfmt,
    ];

    /// The name used both as the bash function name and as argv[1] dispatch.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Hello   => "hello",
            Command::Histfmt => "histfmt",
        }
    }
}
