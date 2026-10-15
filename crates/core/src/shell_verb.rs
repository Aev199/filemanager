//! Commands of plain registry verbs ("Open PowerShell here", "Git Bash
//! here", "Open with Code") for a folder. Explorer fills `%V` from its view;
//! Papka runs such commands itself so they get the right folder.

/// The program and its raw arguments of `template` for `folder`, or `None`
/// when the template uses placeholders that need Explorer itself.
pub fn expand(template: &str, folder: &str) -> Option<(String, String)> {
    let mut line = String::with_capacity(template.len() + folder.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' { line.push(c); continue; }
        match chars.next()? {
            'V' | 'v' | 'W' | 'w' | 'L' | 'l' | '1' => line.push_str(folder),
            '*' => {}
            '%' => line.push('%'),
            _ => return None,
        }
    }
    let line = line.trim();
    let (program, rest) = if let Some(quoted) = line.strip_prefix('"') {
        let end = quoted.find('"')?;
        (&quoted[..end], &quoted[end + 1..])
    } else {
        line.split_once(char::is_whitespace).unwrap_or((line, ""))
    };
    (!program.is_empty()).then(|| (program.to_string(), rest.trim_start().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_background_commands() {
        assert_eq!(
            expand(r#"powershell.exe -noexit -command Set-Location -literalPath '%V'"#, r"D:\Проекты\Сваи"),
            Some(("powershell.exe".into(), r#"-noexit -command Set-Location -literalPath 'D:\Проекты\Сваи'"#.into())));
        assert_eq!(
            expand(r#""C:\Program Files\Git\git-bash.exe" "--cd=%v.""#, r"D:\work"),
            Some((r"C:\Program Files\Git\git-bash.exe".into(), r#""--cd=D:\work.""#.into())));
        assert_eq!(
            expand(r#""C:\Users\a\AppData\Local\Programs\Microsoft VS Code\Code.exe" "%V""#, r"\\server\share"),
            Some((r"C:\Users\a\AppData\Local\Programs\Microsoft VS Code\Code.exe".into(), r#""\\server\share""#.into())));
    }

    #[test]
    fn unknown_placeholders_and_empty_commands_are_left_to_explorer() {
        assert_eq!(expand("app.exe %D", "x"), None);
        assert_eq!(expand("  ", "x"), None);
        assert_eq!(expand(r#""unterminated %V"#, "x"), None);
        assert_eq!(expand("cmd.exe /k echo 100%%", "x"), Some(("cmd.exe".into(), "/k echo 100%".into())));
    }
}
