impl StateMachine {
    fn test_submodule_short_line(&self) -> bool {
        match self.state {
            State::HunkHeader(_, _, _, _) => {
                self.line.starts_with("-Subproject commit ")
                    || self.line.starts_with("+Subproject commit ")
            }
            State::SubmoduleShort(_) => self.line.starts_with("+Subproject commit "),
            _ => false,
        }
    }
}
