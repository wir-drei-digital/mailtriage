//! Numbered prompts for `setup`, standard library only. Questions go to one
//! stream (stderr); answers are read from another (stdin) one byte at a
//! time, so a child process that inherits stdin sees exactly the bytes no
//! answer has consumed.
use crate::service::err;
use anyhow::Result;
use std::io::{Read, Write};

pub struct Prompter<'a> {
    input: Box<dyn Read + 'a>,
    output: Box<dyn Write + 'a>,
    enabled: bool,
}

impl<'a> Prompter<'a> {
    pub fn new(input: impl Read + 'a, output: impl Write + 'a, enabled: bool) -> Self {
        Self {
            input: Box::new(input),
            output: Box::new(output),
            enabled,
        }
    }

    /// Whether questions are asked; otherwise callers use flags and defaults.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// A line of progress or explanation; written with or without prompts.
    pub fn say(&mut self, text: &str) {
        let _ = writeln!(self.output, "{text}");
    }

    /// One answer, trimmed. End of input before any byte aborts setup.
    fn line(&mut self) -> Result<String> {
        let _ = self.output.flush();
        let mut bytes = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            match self.input.read(&mut byte) {
                Ok(0) if bytes.is_empty() => return Err(err(2, "setup aborted: input ended")),
                Ok(0) => break,
                Ok(_) if byte[0] == b'\n' => break,
                Ok(_) => bytes.push(byte[0]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return Err(err(2, "setup aborted: input ended")),
            }
        }
        Ok(String::from_utf8_lossy(&bytes).trim().to_owned())
    }

    /// Asks until `check` accepts the answer; Enter takes `default`.
    pub fn ask(
        &mut self,
        question: &str,
        default: Option<&str>,
        check: impl Fn(&str) -> Result<String, String>,
    ) -> Result<String> {
        loop {
            match default.filter(|d| !d.is_empty()) {
                Some(d) => {
                    let _ = write!(self.output, "{question} [{d}]: ");
                }
                None => {
                    let _ = write!(self.output, "{question}: ");
                }
            }
            let answer = self.line()?;
            let answer = if answer.is_empty() {
                default.unwrap_or("").to_owned()
            } else {
                answer
            };
            match check(&answer) {
                Ok(value) => return Ok(value),
                Err(reason) => self.say(&format!("  {reason}")),
            }
        }
    }

    pub fn confirm(&mut self, question: &str, default: bool) -> Result<bool> {
        let hint = if default { "Y/n" } else { "y/N" };
        loop {
            let _ = write!(self.output, "{question} [{hint}]: ");
            match self.line()?.to_ascii_lowercase().as_str() {
                "" => return Ok(default),
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                _ => self.say("  Please answer y or n."),
            }
        }
    }

    /// `confirm`, except that the end of input answers `default`.
    pub fn confirm_or(&mut self, question: &str, default: bool) -> bool {
        self.confirm(question, default).unwrap_or(default)
    }

    /// A numbered menu (1-based on screen); returns the 0-based index.
    pub fn choose(&mut self, question: &str, options: &[String], default: usize) -> Result<usize> {
        self.menu(question, options, &[default]);
        let n = options.len();
        loop {
            let _ = write!(self.output, "Choose 1-{n} [{}]: ", default + 1);
            let answer = self.line()?;
            if answer.is_empty() {
                return Ok(default);
            }
            match answer.parse::<usize>() {
                Ok(k) if (1..=n).contains(&k) => return Ok(k - 1),
                _ => self.say(&format!("  Please enter a number from 1 to {n}.")),
            }
        }
    }

    /// Several entries, numbers separated by commas or spaces; returns
    /// sorted, unique 0-based indices.
    pub fn choose_many(
        &mut self,
        question: &str,
        options: &[String],
        defaults: &[usize],
    ) -> Result<Vec<usize>> {
        self.menu(question, options, defaults);
        let n = options.len();
        let shown: Vec<String> = defaults.iter().map(|d| (d + 1).to_string()).collect();
        loop {
            let _ = write!(
                self.output,
                "Choose one or more of 1-{n}, separated by commas [{}]: ",
                shown.join(",")
            );
            let answer = self.line()?;
            if answer.is_empty() {
                return Ok(defaults.to_vec());
            }
            let picked: Option<std::collections::BTreeSet<usize>> = answer
                .split([',', ' '])
                .filter(|part| !part.is_empty())
                .map(|part| {
                    part.parse::<usize>()
                        .ok()
                        .filter(|k| (1..=n).contains(k))
                        .map(|k| k - 1)
                })
                .collect();
            match picked {
                Some(set) if !set.is_empty() => return Ok(set.into_iter().collect()),
                _ => self.say(&format!(
                    "  Please enter numbers from 1 to {n}, for example 1,3."
                )),
            }
        }
    }

    fn menu(&mut self, question: &str, options: &[String], marked: &[usize]) {
        self.say(question);
        for (i, option) in options.iter().enumerate() {
            let mark = if marked.contains(&i) {
                " (default)"
            } else {
                ""
            };
            self.say(&format!("  {}) {option}{mark}", i + 1));
        }
    }
}

/// Stdin without the standard library's buffer, so the bytes after an
/// answer stay for a child process that inherits stdin.
pub fn stdin_unbuffered() -> Box<dyn Read> {
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;
        if let Ok(fd) = std::io::stdin().as_fd().try_clone_to_owned() {
            return Box::new(std::fs::File::from(fd));
        }
    }
    Box::new(std::io::stdin())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::ServiceError;

    fn check(answer: &str) -> Result<String, String> {
        if answer.contains(' ') {
            Err("No spaces, please.".to_owned())
        } else {
            Ok(answer.to_owned())
        }
    }

    #[test]
    fn enter_takes_the_default_and_invalid_answers_are_asked_again() {
        let mut out = Vec::new();
        let mut p = Prompter::new(&b"\nbad name\nok\n"[..], &mut out, true);
        assert_eq!(p.ask("Name", Some("work"), check).unwrap(), "work");
        assert_eq!(p.ask("Name", None, check).unwrap(), "ok");
        drop(p);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Name [work]: "), "{text}");
        assert!(text.contains("  No spaces, please."), "{text}");
    }

    #[test]
    fn menus_are_numbered_and_ask_again_when_out_of_range() {
        let mut out = Vec::new();
        let options = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let mut p = Prompter::new(&b"9\n2\n\n3 1\nx\n2,2\n"[..], &mut out, true);
        assert_eq!(p.choose("Pick", &options, 0).unwrap(), 1);
        assert_eq!(p.choose_many("Pick", &options, &[0]).unwrap(), vec![0]);
        assert_eq!(p.choose_many("Pick", &options, &[0]).unwrap(), vec![0, 2]);
        assert_eq!(p.choose_many("Pick", &options, &[0]).unwrap(), vec![1]);
        drop(p);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("  1) a (default)"), "{text}");
        assert!(
            text.contains("Please enter a number from 1 to 3."),
            "{text}"
        );
        assert!(text.contains("Please enter numbers from 1 to 3"), "{text}");
    }

    #[test]
    fn confirm_accepts_yes_no_and_the_default() {
        let mut out = Vec::new();
        let mut p = Prompter::new(&b"\nn\nmaybe\nYES\n"[..], &mut out, true);
        assert!(p.confirm("Go?", true).unwrap());
        assert!(!p.confirm("Go?", true).unwrap());
        assert!(p.confirm("Go?", false).unwrap());
        drop(p);
        assert!(String::from_utf8(out).unwrap().contains("Go? [y/N]: "));
    }

    #[test]
    fn end_of_input_aborts_with_exit_2() {
        let mut out = Vec::new();
        let mut p = Prompter::new(&b""[..], &mut out, true);
        let error = p.ask("Name", Some("x"), check).unwrap_err();
        let error = error.downcast_ref::<ServiceError>().unwrap();
        assert_eq!(
            (error.code, error.message.as_str()),
            (2, "setup aborted: input ended")
        );
    }

    #[test]
    fn answers_are_read_without_reading_ahead() {
        let mut input: &[u8] = b"first\nrest for a child\n";
        let mut out = Vec::new();
        let mut p = Prompter::new(&mut input, &mut out, true);
        assert_eq!(p.ask("Q", None, check).unwrap(), "first");
        drop(p);
        assert_eq!(input, b"rest for a child\n");
    }
}
