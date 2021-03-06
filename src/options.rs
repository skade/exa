use std::cmp;
use std::fmt;
use std::num::ParseIntError;
use std::os::unix::fs::MetadataExt;

use getopts;
use natord;

use colours::Colours;
use column::Column;
use column::Column::*;
use dir::Dir;
use feature::xattr;
use file::File;
use output::{Grid, Details, GridDetails, Lines};
use term::dimensions;


/// The *Options* struct represents a parsed version of the user's
/// command-line options.
#[derive(PartialEq, Debug, Copy, Clone)]
pub struct Options {
    pub dir_action: DirAction,
    pub filter: FileFilter,
    pub view: View,
}

impl Options {

    /// Call getopts on the given slice of command-line strings.
    #[allow(unused_results)]
    pub fn getopts(args: &[String]) -> Result<(Options, Vec<String>), Misfire> {
        let mut opts = getopts::Options::new();
        opts.optflag("1", "oneline",   "display one entry per line");
        opts.optflag("a", "all",       "show dot-files");
        opts.optflag("b", "binary",    "use binary prefixes in file sizes");
        opts.optflag("B", "bytes",     "list file sizes in bytes, without prefixes");
        opts.optflag("d", "list-dirs", "list directories as regular files");
        opts.optflag("g", "group",     "show group as well as user");
        opts.optflag("G", "grid",      "display entries in a grid view (default)");
        opts.optflag("",  "group-directories-first", "list directories before other files");
        opts.optflag("h", "header",    "show a header row at the top");
        opts.optflag("H", "links",     "show number of hard links");
        opts.optflag("i", "inode",     "show each file's inode number");
        opts.optflag("l", "long",      "display extended details and attributes");
        opts.optopt ("L", "level",     "maximum depth of recursion", "DEPTH");
        opts.optflag("m", "modified",  "display timestamp of most recent modification");
        opts.optflag("r", "reverse",   "reverse order of files");
        opts.optflag("R", "recurse",   "recurse into directories");
        opts.optopt ("s", "sort",      "field to sort by", "WORD");
        opts.optflag("S", "blocks",    "show number of file system blocks");
        opts.optopt ("t", "time",      "which timestamp to show for a file", "WORD");
        opts.optflag("T", "tree",      "recurse into subdirectories in a tree view");
        opts.optflag("u", "accessed",  "display timestamp of last access for a file");
        opts.optflag("U", "created",   "display timestamp of creation for a file");
        opts.optflag("x", "across",    "sort multi-column view entries across");

        opts.optflag("",  "version",   "display version of exa");
        opts.optflag("?", "help",      "show list of command-line options");

        if cfg!(feature="git") {
            opts.optflag("", "git", "show git status");
        }

        if xattr::ENABLED {
            opts.optflag("@", "extended", "display extended attribute keys and sizes in long (-l) output");
        }

        let matches = match opts.parse(args) {
            Ok(m)   => m,
            Err(e)  => return Err(Misfire::InvalidOptions(e)),
        };

        if matches.opt_present("help") {
            return Err(Misfire::Help(opts.usage("Usage:\n  exa [options] [files...]")));
        }
        else if matches.opt_present("version") {
            return Err(Misfire::Version);
        }

        let sort_field = match matches.opt_str("sort") {
            Some(word)  => try!(SortField::from_word(word)),
            None        => SortField::default(),
        };

        let filter = FileFilter {
            list_dirs_first: matches.opt_present("group-directories-first"),
            reverse:         matches.opt_present("reverse"),
            show_invisibles: matches.opt_present("all"),
            sort_field:      sort_field,
        };

        let path_strs = if matches.free.is_empty() {
            vec![ ".".to_string() ]
        }
        else {
            matches.free.clone()
        };

        let dir_action = try!(DirAction::deduce(&matches));
        let view = try!(View::deduce(&matches, filter, dir_action));

        Ok((Options {
            dir_action: dir_action,
            view:       view,
            filter:     filter,
        }, path_strs))
    }

    pub fn sort_files(&self, files: &mut Vec<File>) {
        self.filter.sort_files(files)
    }

    pub fn filter_files(&self, files: &mut Vec<File>) {
        self.filter.filter_files(files)
    }

    /// Whether the View specified in this set of options includes a Git
    /// status column. It's only worth trying to discover a repository if the
    /// results will end up being displayed.
    pub fn should_scan_for_git(&self) -> bool {
        match self.view {
            View::Details(Details { columns: Some(cols), .. }) => cols.should_scan_for_git(),
            View::GridDetails(GridDetails { details: Details { columns: Some(cols), .. }, .. }) => cols.should_scan_for_git(),
            _ => false,
        }
    }
}


#[derive(Default, PartialEq, Debug, Copy, Clone)]
pub struct FileFilter {
    list_dirs_first: bool,
    reverse: bool,
    show_invisibles: bool,
    sort_field: SortField,
}

impl FileFilter {
    pub fn filter_files(&self, files: &mut Vec<File>) {
        if !self.show_invisibles {
            files.retain(|f| !f.is_dotfile());
        }
    }

    pub fn sort_files(&self, files: &mut Vec<File>) {
        files.sort_by(|a, b| self.compare_files(a, b));

        if self.reverse {
            files.reverse();
        }

        if self.list_dirs_first {
            // This relies on the fact that sort_by is stable.
            files.sort_by(|a, b| b.is_directory().cmp(&a.is_directory()));
        }
    }

    pub fn compare_files(&self, a: &File, b: &File) -> cmp::Ordering {
        match self.sort_field {
            SortField::Unsorted      => cmp::Ordering::Equal,
            SortField::Name          => natord::compare(&*a.name, &*b.name),
            SortField::Size          => a.metadata.len().cmp(&b.metadata.len()),
            SortField::FileInode     => a.metadata.ino().cmp(&b.metadata.ino()),
            SortField::ModifiedDate  => a.metadata.mtime().cmp(&b.metadata.mtime()),
            SortField::AccessedDate  => a.metadata.atime().cmp(&b.metadata.atime()),
            SortField::CreatedDate   => a.metadata.ctime().cmp(&b.metadata.ctime()),
            SortField::Extension     => match a.ext.cmp(&b.ext) {
                cmp::Ordering::Equal  => natord::compare(&*a.name, &*b.name),
                order                 => order,
            },
        }
    }
}

/// User-supplied field to sort by.
#[derive(PartialEq, Debug, Copy, Clone)]
pub enum SortField {
    Unsorted, Name, Extension, Size, FileInode,
    ModifiedDate, AccessedDate, CreatedDate,
}

impl Default for SortField {
    fn default() -> SortField {
        SortField::Name
    }
}

impl SortField {

    /// Find which field to use based on a user-supplied word.
    fn from_word(word: String) -> Result<SortField, Misfire> {
        match &word[..] {
            "name" | "filename"   => Ok(SortField::Name),
            "size" | "filesize"   => Ok(SortField::Size),
            "ext"  | "extension"  => Ok(SortField::Extension),
            "mod"  | "modified"   => Ok(SortField::ModifiedDate),
            "acc"  | "accessed"   => Ok(SortField::AccessedDate),
            "cr"   | "created"    => Ok(SortField::CreatedDate),
            "none"                => Ok(SortField::Unsorted),
            "inode"               => Ok(SortField::FileInode),
            field                 => Err(SortField::none(field))
        }
    }

    /// How to display an error when the word didn't match with anything.
    fn none(field: &str) -> Misfire {
        Misfire::InvalidOptions(getopts::Fail::UnrecognizedOption(format!("--sort {}", field)))
    }
}


/// One of these things could happen instead of listing files.
#[derive(PartialEq, Debug)]
pub enum Misfire {

    /// The getopts crate didn't like these arguments.
    InvalidOptions(getopts::Fail),

    /// The user asked for help. This isn't strictly an error, which is why
    /// this enum isn't named Error!
    Help(String),

    /// The user wanted the version number.
    Version,

    /// Two options were given that conflict with one another.
    Conflict(&'static str, &'static str),

    /// An option was given that does nothing when another one either is or
    /// isn't present.
    Useless(&'static str, bool, &'static str),

    /// An option was given that does nothing when either of two other options
    /// are not present.
    Useless2(&'static str, &'static str, &'static str),

    /// A numeric option was given that failed to be parsed as a number.
    FailedParse(ParseIntError),
}

impl Misfire {
    /// The OS return code this misfire should signify.
    pub fn error_code(&self) -> i32 {
        if let Misfire::Help(_) = *self { 2 }
                                   else { 3 }
    }
}

impl fmt::Display for Misfire {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use self::Misfire::*;

        match *self {
            InvalidOptions(ref e)  => write!(f, "{}", e),
            Help(ref text)         => write!(f, "{}", text),
            Version                => write!(f, "exa {}", env!("CARGO_PKG_VERSION")),
            Conflict(a, b)         => write!(f, "Option --{} conflicts with option {}.", a, b),
            Useless(a, false, b)   => write!(f, "Option --{} is useless without option --{}.", a, b),
            Useless(a, true, b)    => write!(f, "Option --{} is useless given option --{}.", a, b),
            Useless2(a, b1, b2)    => write!(f, "Option --{} is useless without options --{} or --{}.", a, b1, b2),
            FailedParse(ref e)     => write!(f, "Failed to parse number: {}", e),
        }
    }
}


#[derive(PartialEq, Debug, Copy, Clone)]
pub enum View {
    Details(Details),
    Grid(Grid),
    GridDetails(GridDetails),
    Lines(Lines),
}

impl View {
    pub fn deduce(matches: &getopts::Matches, filter: FileFilter, dir_action: DirAction) -> Result<View, Misfire> {
        use self::Misfire::*;

        let long = || {
            if matches.opt_present("across") && !matches.opt_present("grid") {
                Err(Useless("across", true, "long"))
            }
            else if matches.opt_present("oneline") {
                Err(Useless("oneline", true, "long"))
            }
            else {
                let details = Details {
                    columns: Some(try!(Columns::deduce(matches))),
                    header: matches.opt_present("header"),
                    recurse: dir_action.recurse_options(),
                    filter: filter,
                    xattr: xattr::ENABLED && matches.opt_present("extended"),
                    colours: if dimensions().is_some() { Colours::colourful() } else { Colours::plain() },
                };

                Ok(details)
            }
        };

        let long_options_scan = || {
            for option in &[ "binary", "bytes", "inode", "links", "header", "blocks", "time", "group" ] {
                if matches.opt_present(option) {
                    return Err(Useless(option, false, "long"));
                }
            }

            if cfg!(feature="git") && matches.opt_present("git") {
                Err(Useless("git", false, "long"))
            }
            else if matches.opt_present("level") && !matches.opt_present("recurse") && !matches.opt_present("tree") {
                Err(Useless2("level", "recurse", "tree"))
            }
            else if xattr::ENABLED && matches.opt_present("extended") {
                Err(Useless("extended", false, "long"))
            }
            else {
                Ok(())
            }
        };

        let other_options_scan = || {
            if let Some((width, _)) = dimensions() {
                if matches.opt_present("oneline") {
                    if matches.opt_present("across") {
                        Err(Useless("across", true, "oneline"))
                    }
                    else {
                        let lines = Lines {
                             colours: Colours::colourful(),
                        };

                        Ok(View::Lines(lines))
                    }
                }
                else if matches.opt_present("tree") {
                    let details = Details {
                        columns: None,
                        header: false,
                        recurse: dir_action.recurse_options(),
                        filter: filter,
                        xattr: false,
                        colours: if dimensions().is_some() { Colours::colourful() } else { Colours::plain() },
                    };

                    Ok(View::Details(details))
                }
                else {
                    let grid = Grid {
                        across: matches.opt_present("across"),
                        console_width: width,
                        colours: Colours::colourful(),
                    };

                    Ok(View::Grid(grid))
                }
            }
            else {
                // If the terminal width couldn't be matched for some reason, such
                // as the program's stdout being connected to a file, then
                // fallback to the lines view.
                let lines = Lines {
                     colours: Colours::plain(),
                };

                Ok(View::Lines(lines))
            }
        };

        if matches.opt_present("long") {
            let long_options = try!(long());

            if matches.opt_present("grid") {
                match other_options_scan() {
                    Ok(View::Grid(grid)) => return Ok(View::GridDetails(GridDetails { grid: grid, details: long_options })),
                    Ok(lines)            => return Ok(lines),
                    Err(e)               => return Err(e),
                };
            }
            else {
                return Ok(View::Details(long_options));
            }
        }

        try!(long_options_scan());

        other_options_scan()
    }
}


#[derive(PartialEq, Debug, Copy, Clone)]
pub enum SizeFormat {
    DecimalBytes,
    BinaryBytes,
    JustBytes,
}

impl Default for SizeFormat {
    fn default() -> SizeFormat {
        SizeFormat::DecimalBytes
    }
}

impl SizeFormat {
    pub fn deduce(matches: &getopts::Matches) -> Result<SizeFormat, Misfire> {
        let binary = matches.opt_present("binary");
        let bytes  = matches.opt_present("bytes");

        match (binary, bytes) {
            (true,  true )  => Err(Misfire::Conflict("binary", "bytes")),
            (true,  false)  => Ok(SizeFormat::BinaryBytes),
            (false, true )  => Ok(SizeFormat::JustBytes),
            (false, false)  => Ok(SizeFormat::DecimalBytes),
        }
    }
}


#[derive(PartialEq, Debug, Copy, Clone)]
pub enum TimeType {
    FileAccessed,
    FileModified,
    FileCreated,
}

impl TimeType {
    pub fn header(&self) -> &'static str {
        match *self {
            TimeType::FileAccessed  => "Date Accessed",
            TimeType::FileModified  => "Date Modified",
            TimeType::FileCreated   => "Date Created",
        }
    }
}


#[derive(PartialEq, Debug, Copy, Clone)]
pub struct TimeTypes {
    accessed: bool,
    modified: bool,
    created:  bool,
}

impl Default for TimeTypes {
    fn default() -> TimeTypes {
        TimeTypes { accessed: false, modified: true, created: false }
    }
}

impl TimeTypes {

    /// Find which field to use based on a user-supplied word.
    fn deduce(matches: &getopts::Matches) -> Result<TimeTypes, Misfire> {
        let possible_word = matches.opt_str("time");
        let modified = matches.opt_present("modified");
        let created  = matches.opt_present("created");
        let accessed = matches.opt_present("accessed");

        if let Some(word) = possible_word {
            if modified {
                return Err(Misfire::Useless("modified", true, "time"));
            }
            else if created {
                return Err(Misfire::Useless("created", true, "time"));
            }
            else if accessed {
                return Err(Misfire::Useless("accessed", true, "time"));
            }

            match &word[..] {
                "mod" | "modified"  => Ok(TimeTypes { accessed: false, modified: true, created: false }),
                "acc" | "accessed"  => Ok(TimeTypes { accessed: true, modified: false, created: false }),
                "cr"  | "created"   => Ok(TimeTypes { accessed: false, modified: false, created: true }),
                field   => Err(TimeTypes::none(field)),
            }
        }
        else {
            if modified || created || accessed {
                Ok(TimeTypes { accessed: accessed, modified: modified, created: created })
            }
            else {
                Ok(TimeTypes::default())
            }
        }
    }

    /// How to display an error when the word didn't match with anything.
    fn none(field: &str) -> Misfire {
        Misfire::InvalidOptions(getopts::Fail::UnrecognizedOption(format!("--time {}", field)))
    }
}


/// What to do when encountering a directory?
#[derive(PartialEq, Debug, Copy, Clone)]
pub enum DirAction {
    AsFile,
    List,
    Recurse(RecurseOptions),
}

impl DirAction {
    pub fn deduce(matches: &getopts::Matches) -> Result<DirAction, Misfire> {
        let recurse = matches.opt_present("recurse");
        let list    = matches.opt_present("list-dirs");
        let tree    = matches.opt_present("tree");

        match (recurse, list, tree) {
            (true,  true,  _    )  => Err(Misfire::Conflict("recurse", "list-dirs")),
            (_,     true,  true )  => Err(Misfire::Conflict("tree", "list-dirs")),
            (true,  false, false)  => Ok(DirAction::Recurse(try!(RecurseOptions::deduce(matches, false)))),
            (_   ,  _,     true )  => Ok(DirAction::Recurse(try!(RecurseOptions::deduce(matches, true)))),
            (false, true,  _    )  => Ok(DirAction::AsFile),
            (false, false, _    )  => Ok(DirAction::List),
        }
    }

    pub fn recurse_options(&self) -> Option<RecurseOptions> {
        match *self {
            DirAction::Recurse(opts) => Some(opts),
            _ => None,
        }
    }

    pub fn treat_dirs_as_files(&self) -> bool {
        match *self {
            DirAction::AsFile => true,
            DirAction::Recurse(RecurseOptions { tree, .. }) => tree,
            _ => false,
        }
    }
}


#[derive(PartialEq, Debug, Copy, Clone)]
pub struct RecurseOptions {
    pub tree:      bool,
    pub max_depth: Option<usize>,
}

impl RecurseOptions {
    pub fn deduce(matches: &getopts::Matches, tree: bool) -> Result<RecurseOptions, Misfire> {
        let max_depth = if let Some(level) = matches.opt_str("level") {
            match level.parse() {
                Ok(l)  => Some(l),
                Err(e) => return Err(Misfire::FailedParse(e)),
            }
        }
        else {
            None
        };

        Ok(RecurseOptions {
            tree: tree,
            max_depth: max_depth,
        })
    }

    pub fn is_too_deep(&self, depth: usize) -> bool {
        match self.max_depth {
            None    => false,
            Some(d) => {
                d <= depth
            }
        }
    }
}


#[derive(PartialEq, Copy, Clone, Debug, Default)]
pub struct Columns {
    size_format: SizeFormat,
    time_types: TimeTypes,
    inode: bool,
    links: bool,
    blocks: bool,
    group: bool,
    git: bool
}

impl Columns {
    pub fn deduce(matches: &getopts::Matches) -> Result<Columns, Misfire> {
        Ok(Columns {
            size_format: try!(SizeFormat::deduce(matches)),
            time_types:  try!(TimeTypes::deduce(matches)),
            inode:  matches.opt_present("inode"),
            links:  matches.opt_present("links"),
            blocks: matches.opt_present("blocks"),
            group:  matches.opt_present("group"),
            git:    cfg!(feature="git") && matches.opt_present("git"),
        })
    }

    pub fn should_scan_for_git(&self) -> bool {
        self.git
    }

    pub fn for_dir(&self, dir: Option<&Dir>) -> Vec<Column> {
        let mut columns = vec![];

        if self.inode {
            columns.push(Inode);
        }

        columns.push(Permissions);

        if self.links {
            columns.push(HardLinks);
        }

        columns.push(FileSize(self.size_format));

        if self.blocks {
            columns.push(Blocks);
        }

        columns.push(User);

        if self.group {
            columns.push(Group);
        }

        if self.time_types.modified {
            columns.push(Timestamp(TimeType::FileModified));
        }

        if self.time_types.created {
            columns.push(Timestamp(TimeType::FileCreated));
        }

        if self.time_types.accessed {
            columns.push(Timestamp(TimeType::FileAccessed));
        }

        if cfg!(feature="git") {
            if let Some(d) = dir {
                if self.should_scan_for_git() && d.has_git_repo() {
                    columns.push(GitStatus);
                }
            }
        }

        columns
    }
}

#[cfg(test)]
mod test {
    use super::Options;
    use super::Misfire;
    use feature::xattr;

    fn is_helpful<T>(misfire: Result<T, Misfire>) -> bool {
        match misfire {
            Err(Misfire::Help(_)) => true,
            _                     => false,
        }
    }

    #[test]
    fn help() {
        let opts = Options::getopts(&[ "--help".to_string() ]);
        assert!(is_helpful(opts))
    }

    #[test]
    fn help_with_file() {
        let opts = Options::getopts(&[ "--help".to_string(), "me".to_string() ]);
        assert!(is_helpful(opts))
    }

    #[test]
    fn files() {
        let args = Options::getopts(&[ "this file".to_string(), "that file".to_string() ]).unwrap().1;
        assert_eq!(args, vec![ "this file".to_string(), "that file".to_string() ])
    }

    #[test]
    fn no_args() {
        let args = Options::getopts(&[]).unwrap().1;
        assert_eq!(args, vec![ ".".to_string() ])
    }

    #[test]
    fn file_sizes() {
        let opts = Options::getopts(&[ "--long".to_string(), "--binary".to_string(), "--bytes".to_string() ]);
        assert_eq!(opts.unwrap_err(), Misfire::Conflict("binary", "bytes"))
    }

    #[test]
    fn just_binary() {
        let opts = Options::getopts(&[ "--binary".to_string() ]);
        assert_eq!(opts.unwrap_err(), Misfire::Useless("binary", false, "long"))
    }

    #[test]
    fn just_bytes() {
        let opts = Options::getopts(&[ "--bytes".to_string() ]);
        assert_eq!(opts.unwrap_err(), Misfire::Useless("bytes", false, "long"))
    }

    #[test]
    fn long_across() {
        let opts = Options::getopts(&[ "--long".to_string(), "--across".to_string() ]);
        assert_eq!(opts.unwrap_err(), Misfire::Useless("across", true, "long"))
    }

    #[test]
    fn oneline_across() {
        let opts = Options::getopts(&[ "--oneline".to_string(), "--across".to_string() ]);
        assert_eq!(opts.unwrap_err(), Misfire::Useless("across", true, "oneline"))
    }

    #[test]
    fn just_header() {
        let opts = Options::getopts(&[ "--header".to_string() ]);
        assert_eq!(opts.unwrap_err(), Misfire::Useless("header", false, "long"))
    }

    #[test]
    fn just_group() {
        let opts = Options::getopts(&[ "--group".to_string() ]);
        assert_eq!(opts.unwrap_err(), Misfire::Useless("group", false, "long"))
    }

    #[test]
    fn just_inode() {
        let opts = Options::getopts(&[ "--inode".to_string() ]);
        assert_eq!(opts.unwrap_err(), Misfire::Useless("inode", false, "long"))
    }

    #[test]
    fn just_links() {
        let opts = Options::getopts(&[ "--links".to_string() ]);
        assert_eq!(opts.unwrap_err(), Misfire::Useless("links", false, "long"))
    }

    #[test]
    fn just_blocks() {
        let opts = Options::getopts(&[ "--blocks".to_string() ]);
        assert_eq!(opts.unwrap_err(), Misfire::Useless("blocks", false, "long"))
    }

    #[test]
    #[cfg(feature="git")]
    fn just_git() {
        let opts = Options::getopts(&[ "--git".to_string() ]);
        assert_eq!(opts.unwrap_err(), Misfire::Useless("git", false, "long"))
    }

    #[test]
    fn extended_without_long() {
        if xattr::ENABLED {
            let opts = Options::getopts(&[ "--extended".to_string() ]);
            assert_eq!(opts.unwrap_err(), Misfire::Useless("extended", false, "long"))
        }
    }

    #[test]
    fn level_without_recurse_or_tree() {
        let opts = Options::getopts(&[ "--level".to_string(), "69105".to_string() ]);
        assert_eq!(opts.unwrap_err(), Misfire::Useless2("level", "recurse", "tree"))
    }
}
