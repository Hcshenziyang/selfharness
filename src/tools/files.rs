use cap_fs_ext::{DirExt, FollowSymlinks, MetadataExt, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, File, Metadata, OpenOptions},
};
use cap_tempfile::TempFile;
use std::{
    ffi::OsStr,
    io::{self, ErrorKind, Read, Write},
    path::{Component, Path},
};

const MAX_BYTES: usize = 64 * 1024;

pub(super) fn normalize_root(root: &str) -> Result<String, String> {
    let root = Path::new(root);
    if !root.is_absolute() {
        return Err("工作目录需要使用绝对路径".into());
    }
    let root = root
        .canonicalize()
        .map_err(|e| format!("无法打开工作目录：{e}"))?;
    check_root(&root)?;
    root.into_os_string()
        .into_string()
        .map_err(|_| "工作目录路径需要使用 UTF-8".into())
}

pub(super) fn check_root(root: &Path) -> Result<(), String> {
    open_root(root)
        .map(drop)
        .map_err(|e| format!("无法打开工作目录：{e}"))
}

// 从文件系统根逐级打开已规范化的工作目录，重启后也拒绝被换成符号链接的目录。
fn open_root(root: &Path) -> io::Result<Dir> {
    let anchor = root
        .ancestors()
        .last()
        .filter(|p| p.is_absolute())
        .ok_or_else(|| io::Error::other("工作目录无效"))?;
    let dir = Dir::open_ambient_dir(anchor, ambient_authority())?;
    descend(dir, root.strip_prefix(anchor).unwrap())
}

fn descend(mut dir: Dir, relative: &Path) -> io::Result<Dir> {
    for component in relative.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(name) => dir = dir.open_dir_nofollow(name)?,
            _ => return Err(io::Error::other("路径超出助手的工作目录")),
        }
    }
    Ok(dir)
}

fn parent<'p>(root: &str, path: &'p str) -> io::Result<(Dir, &'p OsStr)> {
    // Path 会规范化末尾的分隔符和 "."，先按调用者传入的最后一段检查。
    if matches!(path.rsplit(std::path::is_separator).next(), Some("" | ".")) {
        return Err(io::Error::other("请提供文件路径，末尾需要是文件名"));
    }
    let path = Path::new(path);
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("请提供工作目录内的相对文件路径"))?;
    let dir = descend(open_root(Path::new(root))?, path.parent().unwrap())?;
    Ok((dir, name))
}

fn open_regular(dir: &Dir, name: &OsStr, write: bool) -> io::Result<(File, Metadata)> {
    if !dir.symlink_metadata(name)?.is_file() {
        return Err(io::Error::other("仅支持普通文件，无法访问链接或特殊文件"));
    }
    let mut options = OpenOptions::new();
    options
        .read(!write)
        .write(write)
        .follow(FollowSymlinks::No)
        .nonblock(true);
    let file = dir.open_with(name, &options)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(io::Error::other("仅支持普通文件，无法访问链接或特殊文件"));
    }
    Ok((file, metadata))
}

pub(super) fn read(root: &str, path: &str) -> io::Result<String> {
    let (dir, name) = parent(root, path)?;
    let (file, _) = open_regular(&dir, name, false)?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err(io::Error::other("文件超过 64 KiB"));
    }
    String::from_utf8(bytes).map_err(|_| io::Error::other("文件需要使用 UTF-8 编码"))
}

pub(super) fn write(root: &str, path: &str, content: &str) -> io::Result<String> {
    if content.len() > MAX_BYTES {
        return Err(io::Error::other("内容超过 64 KiB"));
    }
    let (dir, name) = parent(root, path)?;
    // 在已打开的父目录里原子替换文件，避免半写入和经由链接修改目录外的目标。
    let permissions = match open_regular(&dir, name, true) {
        Ok((_, metadata)) => Some(metadata.permissions()),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let mut temp = TempFile::new(&dir)?;
    #[cfg(unix)]
    {
        use cap_std::fs::{Permissions, PermissionsExt};
        let mode = permissions.map_or(0o600, |p| p.mode() & 0o777);
        temp.as_file()
            .set_permissions(Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    if let Some(p) = permissions {
        temp.as_file().set_permissions(p)?;
    }
    temp.write_all(content.as_bytes())?;
    temp.replace(name)?;
    Ok(format!("已写入 {} 字节", content.len()))
}
