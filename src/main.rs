#![cfg_attr(windows, windows_subsystem = "windows")]

use ibps::{
    files,
    patch::{self, Format},
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};
#[cfg(windows)]
mod gui_win;
#[cfg(windows)]
mod settings_win;

const USAGE: &str = "iBPS - ROMパッチの適用と作成
使い方:
  ibps                                      GUIを起動（Windows）
  ibps <patch>                              GUIで適用（Windows）
  ibps [-a|--apply] <patch> <rom> [<out>]
  ibps [-c|--create] [フォーマット] <orig> <mod> [<patch>]
  ibps -I|--info <patch>
  ibps -h|--help
  ibps -v|--version
フォーマット: --ips、--ips32、--ups、--bps
動作を省略すると、引数2個は適用、3個は作成です。
出力パッチを省略する場合はフォーマットを指定してください。
「--」以降は、先頭が「-」の引数もファイルパスとして扱います。";

#[derive(Debug, PartialEq)]
enum Command {
    Gui(Option<PathBuf>),
    Help,
    Version,
    Info(PathBuf),
    Apply(PathBuf, PathBuf, Option<PathBuf>),
    Create(Format, PathBuf, PathBuf, PathBuf),
}

fn parse(args: Vec<OsString>) -> Result<Command, String> {
    let mut action = None;
    let mut format = None;
    let mut paths = Vec::new();
    let mut options = true;
    for arg in args {
        if options && arg == "--" {
            options = false;
            continue;
        }
        if options && let Some(arg) = arg.to_str().filter(|a| a.starts_with('-')) {
            let mode = match arg {
                "-a" | "--apply" => Some('a'),
                "-c" | "--create" => Some('c'),
                "-I" | "--info" => Some('i'),
                "-h" | "--help" => Some('h'),
                "-v" | "--version" => Some('v'),
                _ => None,
            };
            if let Some(mode) = mode {
                if action.replace(mode).is_some() {
                    return Err("動作は1つだけ指定してください".into());
                }
            } else {
                let value = match arg {
                    "--ips" => Format::Ips,
                    "--ips32" => Format::Ips32,
                    "--ups" => Format::Ups,
                    "--bps" => Format::Bps,
                    _ => return Err(format!("不明なオプションです: {arg}")),
                };
                if format.replace(value).is_some() {
                    return Err("フォーマットは1つだけ指定してください".into());
                }
            }
            continue;
        }
        paths.push(PathBuf::from(arg));
    }
    let action = action.unwrap_or(match paths.len() {
        0 | 1 if format.is_none() => 'g',
        2 => 'a',
        3 => 'c',
        _ => return Err("引数の数が正しくありません。--helpで使用方法を確認してください".into()),
    });
    if format.is_some() && action != 'c' {
        return Err("フォーマットオプションはパッチ作成で指定してください（--create）".into());
    }
    match (action, paths.as_slice()) {
        ('g', []) => Ok(Command::Gui(None)),
        ('g', [patch]) => Ok(Command::Gui(Some(patch.clone()))),
        ('h', []) => Ok(Command::Help),
        ('v', []) => Ok(Command::Version),
        ('i', [patch]) => Ok(Command::Info(patch.clone())),
        ('a', [patch, input]) => Ok(Command::Apply(patch.clone(), input.clone(), None)),
        ('a', [patch, input, output]) => Ok(Command::Apply(
            patch.clone(),
            input.clone(),
            Some(output.clone()),
        )),
        ('c', [source, target]) => {
            let format =
                format.ok_or("出力パッチを省略する場合はフォーマットを指定してください")?;
            Ok(Command::Create(
                format,
                source.clone(),
                target.clone(),
                target.with_extension(format.extension()),
            ))
        }
        ('c', [source, target, output]) => {
            let format = format.or_else(|| format_from_extension(output)).ok_or(
                "出力パッチのフォーマットを指定してください（--ips、--ips32、--ups、--bps）",
            )?;
            Ok(Command::Create(
                format,
                source.clone(),
                target.clone(),
                output.clone(),
            ))
        }
        _ => Err("引数の数が正しくありません。--helpで使用方法を確認してください".into()),
    }
}

fn format_from_extension(path: &Path) -> Option<Format> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "ips" => Some(Format::Ips),
        "ups" => Some(Format::Ups),
        "bps" => Some(Format::Bps),
        _ => None,
    }
}

fn execute(command: Command) -> Result<i32, String> {
    let warnings = match command {
        Command::Gui(path) => {
            #[cfg(windows)]
            return Ok(gui_win::run(path));
            #[cfg(not(windows))]
            {
                let _ = path;
                println!("{USAGE}");
                return Ok(0);
            }
        }
        Command::Help => {
            println!("{USAGE}");
            return Ok(0);
        }
        Command::Version => {
            println!("iBPS {}", env!("CARGO_PKG_VERSION"));
            return Ok(0);
        }
        Command::Info(path) => {
            print!("{}", patch::info(&files::read(&path)?)?);
            return Ok(0);
        }
        Command::Apply(patch, input, output) => {
            let warnings = files::apply_files(&patch, &input, output.as_deref())?;
            println!(
                "パッチを適用しました: {}",
                output
                    .unwrap_or_else(|| files::default_apply_path(&patch, &input))
                    .display()
            );
            warnings
        }
        Command::Create(format, source, target, output) => {
            let warnings = files::create_files(format, &source, &target, &output)?;
            println!("パッチを作成しました: {}", output.display());
            warnings
        }
    };
    for warning in warnings {
        println!("警告: {warning}");
    }
    Ok(0)
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    #[cfg(windows)]
    if !args.is_empty() {
        gui_win::attach_console();
    }
    let code = match parse(args).and_then(execute) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("エラー: {error}");
            1
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn command(args: &[&str]) -> Result<Command, String> {
        parse(args.iter().map(OsString::from).collect())
    }
    #[test]
    fn actions_and_defaults() {
        assert!(matches!(
            command(&["x.ips", "入力.gba"]).unwrap(),
            Command::Apply(_, _, None)
        ));
        assert!(matches!(
            command(&["old", "new", "out.BPS"]).unwrap(),
            Command::Create(Format::Bps, _, _, _)
        ));
        assert_eq!(
            command(&["-c", "--ips32", "old", "new.gba"]).unwrap(),
            Command::Create(
                Format::Ips32,
                "old".into(),
                "new.gba".into(),
                "new.ips".into()
            )
        );
        assert!(command(&["-a", "-c", "x", "y"]).is_err());
        assert!(command(&["--ups", "--bps", "x", "y", "z"]).is_err());
        assert!(command(&["-c", "x", "y"]).is_err());
        assert!(matches!(
            command(&["-a", "--", "-x", "y"]).unwrap(),
            Command::Apply(_, _, None)
        ));
        assert!(command(&["--unknown"]).is_err());
    }
}
