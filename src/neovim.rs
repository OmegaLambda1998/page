/// A module for actions done with neovim


use neovim_lib::{
    neovim_api::{Buffer, Window},
    NeovimApi, Value,
};
use std::{
    path::PathBuf,
    sync::mpsc,
};


/// This struct wraps neovim_lib::Neovim in order to enhance it with methods required in page.
/// Results returned from underlying Neovim methods are mostly unwrapped, since we anyway cannot provide
/// any meaningful falback logic on call side.
pub struct NeovimActions {
    nvim: neovim_lib::Neovim,
}

impl NeovimActions {
    pub fn on(nvim: neovim_lib::Neovim) -> NeovimActions {
        NeovimActions { nvim }
    }

    pub fn get_current_window_and_buffer(&mut self) -> (Window, Buffer) {
        (self.nvim.get_current_win().unwrap(), self.nvim.get_current_buf().unwrap())
    }

    pub fn get_current_buffer(&mut self) -> Buffer {
        self.nvim.get_current_buf().unwrap()
    }

    pub fn get_buffer_number(&mut self, buf: &Buffer) -> i64 {
        buf.get_number(&mut self.nvim).unwrap()
    }

    pub fn create_substituting_output_buffer(&mut self) -> Buffer {
        self.nvim.command("term tail -f <<EOF").unwrap();
        let buf = self.get_current_buffer();
        log::trace!(target: "new substituting output buffer", "{}", self.get_buffer_number(&buf));
        buf
    }

    pub fn create_split_output_buffer(&mut self, opt: &crate::cli::Options) -> Buffer {
        let cmd = if opt.split_right > 0u8 {
            format!("exe 'belowright ' . ((winwidth(0)/2) * 3 / {}) . 'vsplit term://tail -f <<EOF' | set winfixwidth", opt.split_right + 1)
        } else if opt.split_left > 0u8 {
            format!("exe 'aboveleft ' . ((winwidth(0)/2) * 3 / {}) . 'vsplit term://tail -f <<EOF' | set winfixwidth", opt.split_left + 1)
        } else if opt.split_below > 0u8 {
            format!("exe 'belowright ' . ((winheight(0)/2) * 3 / {}) . 'split term://tail -f <<EOF' | set winfixheight", opt.split_below + 1)
        } else if opt.split_above > 0u8 {
            format!("exe 'aboveleft ' . ((winheight(0)/2) * 3 / {}) . 'split term://tail -f <<EOF' | set winfixheight", opt.split_above + 1)
        } else if let Some(split_right_cols) = opt.split_right_cols {
            format!("belowright {}vsplit term://tail -f <<EOF | set winfixwidth", split_right_cols)
        } else if let Some(split_left_cols) = opt.split_left_cols {
            format!("aboveleft {}vsplit term://tail -f <<EOF | set winfixwidth", split_left_cols)
        } else if let Some(split_below_rows) = opt.split_below_rows {
            format!("belowright {}split term://tail -f <<EOF | set winfixheight", split_below_rows)
        } else if let Some(split_above_rows) = opt.split_above_rows {
            format!("aboveleft {}split term://tail -f <<EOF | set winfixheight", split_above_rows)
        } else {
            "".into()
        };
        log::trace!(target: "split command", "{}", cmd);
        self.nvim.command(&cmd).expect("Error when creating split buffer");
        let buf = self.get_current_buffer();
        log::trace!(target: "new split output buffer", "{}", self.get_buffer_number(&buf));
        buf
    }

    pub fn get_current_buffer_pty_path(&mut self) -> PathBuf {
        let buf_pty_path: PathBuf = self.nvim.eval("nvim_get_chan_info(&channel)").expect("Cannot get channel info")
            .as_map().unwrap()
            .iter().find(|(k, _)| k.as_str().map(|s| s == "pty").unwrap()).expect("Cannot find 'pty' on channel info")
            .1.as_str().unwrap()
            .into();
        log::trace!(target: "use pty", "{}", buf_pty_path.display());
        buf_pty_path
    }

    pub fn mark_buffer_as_instance(&mut self, buffer: &Buffer, inst_name: &str, inst_pty_path: &str) {
        log::trace!(target: "new instance", "{:?}->{}->{}", buffer, inst_name, inst_pty_path);
        let v = Value::from(vec![Value::from(inst_name), Value::from(inst_pty_path)]);
        if let Err(e) = buffer.set_var(&mut self.nvim, "page_instance", v) {
            log::error!(target: "new instance", "Error when setting instance mark: {}", e);
        }
    }

    pub fn find_instance_buffer(&mut self, inst_name: &str) -> Option<(Buffer, PathBuf)> {
        for buf in self.nvim.list_bufs().unwrap() {
            let inst_var = buf.get_var(&mut self.nvim, "page_instance");
            log::trace!(target: "instances", "{:?} => {}: {:?}", buf.get_number(&mut self.nvim), inst_name, inst_var);
            match inst_var {
                Err(e) => {
                    let descr = e.to_string();
                    if descr != "1 - Key 'page_instance' not found"
                    && descr != "1 - Key not found: page_instance" { // For new neovim version
                        panic!("Error when getting instance mark: {}", e);
                    }
                }
                Ok(v) => {
                    if let Some(arr) = v.as_array().map(|a|a.iter().map(Value::as_str).collect::<Vec<_>>()) {
                        if let [Some(inst_name_found), Some(inst_pty_path)] = arr[..] {
                            log::trace!(target: "found instance", "{}->{}", inst_name_found, inst_pty_path);
                            if inst_name == inst_name_found {
                                let sink = PathBuf::from(inst_pty_path.to_string());
                                return Some((buf, sink))
                            }
                        }
                    }
                }
            }
        };
        None
    }

    pub fn close_instance_buffer(&mut self, inst_name: &str) {
        log::trace!(target: "close instance", "{}", inst_name);
        if let Some((buf, _)) = self.find_instance_buffer(&inst_name) {
            if let Err(e) = buf.get_number(&mut self.nvim).and_then(|inst_id| self.nvim.command(&format!("exe 'bd!' . {}", inst_id))) {
                log::error!(target: "close instance", "Error when closing instance buffer: {}, {}", inst_name, e);
            }
        }
    }

    pub fn focus_instance_buffer(&mut self, inst_buf: &Buffer) {
        log::trace!(target: "focus instance", "{:?}", inst_buf);
        if &self.get_current_buffer() != inst_buf {
            let wins_open = self.nvim.list_wins().unwrap();
            log::trace!(target: "focus instance", "Winows open: {:?}", wins_open.iter().map(|w| w.get_number(&mut self.nvim)));
            for win in wins_open {
                if &win.get_buf(&mut self.nvim).unwrap() == inst_buf {
                    log::trace!(target: "focus instance", "Use window: {:?}", win.get_number(&mut self.nvim));
                    self.nvim.set_current_win(&win).unwrap();
                    return
                }
            }
        } else {
            log::trace!(target: "focus instance", "Not in window");
        }
        self.nvim.set_current_buf(inst_buf).unwrap();
    }


    pub fn update_buffer_title(&mut self, buf: &Buffer, buf_title: &str) {
        log::trace!(target: "update title", "{:?} => {}", buf.get_number(&mut self.nvim), buf_title);
        let a = std::iter::once((0, buf_title.to_string()));
        let b = (1..99).map(|attempt_nr| (attempt_nr, format!("{}({})", buf_title, attempt_nr)));
        for (attempt_nr, name) in a.chain(b) {
            match buf.set_name(&mut self.nvim, &name) {
                Err(e) => {
                    log::trace!(target: "update title", "{:?} => {}: {:?}", buf.get_number(&mut self.nvim), buf_title, e);
                    if 99 < attempt_nr || e.to_string() != "0 - Failed to rename buffer" {
                        log::error!(target: "update title", "Cannot update title: {}", e);
                        return
                    }
                }
                _ => {
                    self.nvim.command("redraw!").unwrap();  // To update statusline
                    return
                }
            }
        }
    }

    pub fn prepare_file_buffer(&mut self, cmd_user: &str, initial_buf_nr: i64) {
        let cmd_post = "| exe 'silent doautocmd User PageOpenFile'";
        self.prepare_current_buffer("", cmd_user, "", cmd_post, initial_buf_nr)
    }

    pub fn prepare_output_buffer(&mut self, page_id: &str, ft: &str, cmd_user: &str, pwd: bool, query_lines: u64, initial_buf_nr: i64) {
        let ft = format!("filetype={}", ft);
        let mut cmd_pre = String::new();
        if query_lines > 0u64 {
            let query_opts = format!(" \
                | exe 'command! -nargs=? Page call rpcnotify(0, ''page_fetch_lines'', ''{page_id}'', <args>)' \
                | exe 'autocmd BufEnter <buffer> command! -nargs=? Page call rpcnotify(0, ''page_fetch_lines'', ''{page_id}'', <args>)' \
                | exe 'autocmd BufDelete <buffer> call rpcnotify(0, ''page_buffer_closed'', ''{page_id}'')' \
            ",
                page_id = page_id,
            );
            cmd_pre.push_str(&query_opts);
        }
        if pwd {
            let pwd_opts = format!(" \
                | let b:page_lcd_backup = getcwd() \
                | lcd {pwd} \
                | exe 'autocmd BufEnter <buffer> lcd {pwd}' \
                | exe 'autocmd BufLeave <buffer> lcd ' .. b:page_lcd_backup \
            ",
                pwd = std::env::var("PWD").unwrap()
            );
            cmd_pre.push_str(&pwd_opts);
        }
        self.prepare_current_buffer(&ft, cmd_user, &cmd_pre, "", initial_buf_nr)
    }

    fn prepare_current_buffer(&mut self, ft: &str, cmd_user: &str, cmd_pre: &str, cmd_post: &str, initial_buf_nr: i64) {
        let cmd_user = match cmd_user {
            "" => String::new(),
            _ => format!("| exe '{}'", cmd_user.replace("'", "''")) // Ecranizes viml literal string
        };
        let options = format!(" \
            | let b:page_alternate_bufnr={initial_buf_nr} \
            | let b:page_scrolloff_backup=&scrolloff \
            | setl scrollback=100000 scrolloff=999 signcolumn=no nonumber nomodifiable {ft} \
            | exe 'autocmd BufEnter <buffer> set scrolloff=999' \
            | exe 'autocmd BufLeave <buffer> let &scrolloff=b:page_scrolloff_backup' \
            {cmd_pre} \
            | exe 'silent doautocmd User PageOpen' \
            | redraw \
            {cmd_user} \
            {cmd_post} \
        ",
            initial_buf_nr = initial_buf_nr,
            ft = ft,
            cmd_user = cmd_user,
            cmd_pre = cmd_pre,
            cmd_post = cmd_post,
        );
        log::trace!(target: "prepare output", "{}", options);
        if let Err(e) = self.nvim.command(&options) {
            log::error!(target: "prepare output", "Unable to set page options, text might be displayed improperly: {}", e);
        }
    }

    pub fn execute_connect_autocmd_on_current_buffer(&mut self) {
        log::trace!(target: "au PageConnect", "");
        if let Err(e) = self.nvim.command("silent doautocmd User PageConnect") {
            log::error!(target: "au PageConnect", "Cannot execute PageConnect: {}", e);
        }
    }

    pub fn execute_disconnect_autocmd_on_current_buffer(&mut self) {
        log::trace!(target: "au PageDisconnect", "");
        if let Err(e) = self.nvim.command("silent doautocmd User PageDisconnect") {
            log::error!(target: "au PageDisconnect", "Cannot execute PageDisconnect: {}", e);
        }
    }

    pub fn execute_command_post(&mut self, cmd: &str) {
        log::trace!(target: "command post", "{}", cmd);
        if let Err(e) = self.nvim.command(cmd) {
            log::error!(target: "command post", "Error when executing post command '{}': {}", cmd, e);
        }
    }

    pub fn switch_to_window_and_buffer(&mut self, (win, buf): &(Window, Buffer)) {
        log::trace!(target: "set window and buffer", "win:{:?} buf:{:?}",  win.get_number(&mut self.nvim), buf.get_number(&mut self.nvim));
        if let Err(e) = self.nvim.set_current_win(win) {
            log::error!(target: "set window and buffer", "Can't switch to window: {}", e);
        }
        if let Err(e) = self.nvim.set_current_buf(buf) {
            log::error!(target: "set window and buffer", "Can't switch to buffer: {}", e);
        }
    }

    pub fn switch_to_buffer(&mut self, buf: &Buffer) {
        log::trace!(target: "set buffer", "{:?}", buf.get_number(&mut self.nvim));
        self.nvim.set_current_buf(buf).unwrap();
    }

    pub fn set_current_buffer_insert_mode(&mut self) {
        log::trace!(target: "set INSERT", "");
        if let Err(e) = self.nvim.command(r###"call feedkeys("\<C-\>\<C-n>A", 'n')"###) {// Fixes "can't enter normal mode from..."
            log::error!(target: "set INSERT", "Error when setting mode: {}", e);
        }
    }

    pub fn set_current_buffer_follow_output_mode(&mut self) {
        log::trace!(target: "set FOLLOW", "");
        if let Err(e) = self.nvim.command(r###"call feedkeys("\<C-\>\<C-n>G, 'n'")"###) {
            log::error!(target: "set FOLLOW", "Error when setting mode: {}", e);
        }
    }

    pub fn set_current_buffer_scroll_mode(&mut self) {
        log::trace!(target: "set SCROLL", "");
        if let Err(e) = self.nvim.command(r###"call feedkeys("\<C-\>\<C-n>ggM, 'n'")"###) {
            log::error!(target: "set SCROLL", "Error when setting mode: {}", e);
        }
    }

    pub fn open_file_buffer(&mut self, file_path: &str) -> Result<(), Box<dyn std::error::Error>> {
        log::trace!(target: "open file", "{}", file_path);
        self.nvim.command(&format!("e {}", std::fs::canonicalize(file_path)?.to_string_lossy()))?;
        Ok(())
    }

    pub fn notify_query_finished(&mut self, lines_read: u64) {
        log::trace!(target: "query finished", "");
        self.nvim.command(&format!("redraw | echoh Comment | echom '-- [PAGE] {} lines read; has more --' | echoh None", lines_read)).unwrap();
    }

    pub fn notify_end_of_input(&mut self) {
        log::trace!(target: "end input", "");
        self.nvim.command("redraw | echoh Comment | echom '-- [PAGE] end of input --' | echoh None").unwrap();
    }

    pub fn get_var_or(&mut self, key: &str, default: &str) -> String {
        self.nvim.get_var(key)
            .map(|v| v.to_string())
            .unwrap_or_else(|e| {
                let description = e.to_string();
                if description != format!("1 - Key '{}' not found", key)
                && description != format!("1 - Key not found: {}", key) { // For new neovim version
                    log::error!("Error when getting var: {}, {}", key, e);
                }
                String::from(default)
            })
    }
}



/// This is type-safe enumeration of notifications that could be done from neovim side.
/// Maybe it'll be enhanced in future
pub enum NotificationFromNeovim {
    FetchPart,
    FetchLines(u64),
    BufferClosed,
}

mod notifications {
    use super::NotificationFromNeovim;
    use neovim_lib::{Value, NeovimApi};
    use std::sync::mpsc;

    /// Registers handler which receives notifications from neovim side.
    /// Commands are received on separate thread and further redirected to mpsc sender
    /// associated with receiver returned from current function.
    pub fn subscribe(nvim: &mut neovim_lib::Neovim, page_id: &str) -> mpsc::Receiver<NotificationFromNeovim> {
        log::trace!(target: "subscribe to notifications", "id: {}", page_id);
        let (tx, rx) = mpsc::sync_channel(16);
        nvim.session.start_event_loop_handler(NotificationReceiver { tx, page_id: page_id.to_string() });
        nvim.subscribe("page_fetch_lines").unwrap();
        nvim.subscribe("page_buffer_closed").unwrap();
        rx
    }

    /// Receives and collects notifications from neovim side
    struct NotificationReceiver {
        pub tx: mpsc::SyncSender<NotificationFromNeovim>,
        pub page_id: String,
    }

    impl neovim_lib::Handler for NotificationReceiver {
        fn handle_notify(&mut self, notification: &str, args: Vec<Value>) {
            log::trace!(target: "notification", "{}: {:?} ", notification, args);
            let page_id = args.get(0).and_then(Value::as_str);
            if page_id.map_or(true, |page_id| page_id != self.page_id) {
                log::warn!(target: "invalid page id", "");
                return
            }
            let notification_from_neovim = match notification {
                "page_fetch_lines" => {
                     if let Some(lines_count) = args.get(1).and_then(Value::as_u64) {
                        NotificationFromNeovim::FetchLines(lines_count)
                    } else {
                        NotificationFromNeovim::FetchPart
                    }
                },
                "page_buffer_closed" => {
                    NotificationFromNeovim::BufferClosed
                },
                _ => {
                    log::warn!(target: "unhandled notification", "");
                    return
                }
            };
            self.tx.send(notification_from_neovim).expect("cannot receive notification")
        }
    }

    impl neovim_lib::RequestHandler for NotificationReceiver {
        fn handle_request(&mut self, request: &str, args: Vec<Value>) -> Result<Value, Value> {
            log::warn!(target: "unhandled request", "{}: {:?}", request, args);
            Ok(Value::from(0))
        }
    }
}



/// This struct contains all neovim-related data which is required by page
/// after connection with neovim is established.
pub struct NeovimConnection {
    pub nvim_proc: Option<std::process::Child>,
    pub nvim_actions: NeovimActions,
    pub initial_win_and_buf: (neovim_lib::neovim_api::Window, neovim_lib::neovim_api::Buffer),
    pub initial_buf_number: i64,
    pub rx: mpsc::Receiver<NotificationFromNeovim>,
}

impl NeovimConnection {
    pub fn is_child_neovim_process_spawned(&self) -> bool {
        self.nvim_proc.is_some()
    }
}

pub mod connection {
    use crate::{context, cli::Options};
    use super::{notifications, NeovimConnection, NeovimActions};
    use std::{path::PathBuf, process};

    /// Connects to parent neovim session if possible or spawns new child neovim process and connects to it through socket.
    /// Replacement for `neovim_lib::Session::new_child()`, since it uses --embed flag and steals page stdin.
    pub fn open(cli_ctx: &context::CliContext) -> NeovimConnection {
        let (nvim_session, nvim_proc) = if let Some(nvim_listen_addr) = cli_ctx.opt.address.as_deref() {
            let session_at_addr = session_at_address(nvim_listen_addr).expect("cannot connect to parent neovim");
            (session_at_addr, None)
        } else {
            session_with_new_neovim_process(&cli_ctx)
        };
        let mut nvim = neovim_lib::Neovim::new(nvim_session);
        let rx = notifications::subscribe(&mut nvim, &cli_ctx.page_id);
        let mut nvim_actions = NeovimActions::on(nvim);
        let initial_win_and_buf = nvim_actions.get_current_window_and_buffer();
        let initial_buf_number = nvim_actions.get_buffer_number(&initial_win_and_buf.1);
        NeovimConnection {
            nvim_proc,
            nvim_actions,
            initial_win_and_buf,
            initial_buf_number,
            rx,
        }
    }

    /// Waits until child neovim closes. If no child neovim process then it's safe to exit from page
    pub fn close(nvim_connection: NeovimConnection) {
        if let Some(mut process) = nvim_connection.nvim_proc {
            process.wait().expect("Neovim process died unexpectedly");
        }
    }

    /// Creates a new session using TCP or UNIX socket, or fallbacks to a new neovim process
    /// Also prints redirection protection in appropriate circumstances.
    fn session_with_new_neovim_process(cli_ctx: &context::CliContext) -> (neovim_lib::Session, Option<process::Child>) {
        let context::CliContext { opt, tmp_dir, page_id, print_protection, .. } = cli_ctx;
        if *print_protection {
            print_redirect_protection(&tmp_dir);
        }
        let p = tmp_dir.clone().join(&format!("socket-{}", page_id));
        let nvim_listen_addr = p.to_string_lossy();
        let nvim_proc = spawn_child_nvim_process(opt, &nvim_listen_addr);
        let mut i = 0;
        let e = loop {
            match session_at_address(&nvim_listen_addr) {
                Ok(nvim_session) => return (nvim_session, Some(nvim_proc)),
                Err(e) => {
                    if let std::io::ErrorKind::NotFound = e.kind() {
                        if i == 100 {
                            break e
                        } else {
                            log::trace!(target: "cannot connect to child neovim", "[attempt #{}] address '{}': {:?}", i, nvim_listen_addr, e);
                            std::thread::sleep(std::time::Duration::from_millis(16));
                            i += 1
                        }
                    } else {
                        break e
                    }
                }
            }
        };
        panic!("Cannot connect to neovim: {:?}", e);
    }

    /// Redirecting protection prevents from producing junk or corruption of existed files
    /// by invoking commands like "unset NVIM_LISTEN_ADDRESS && ls > $(page -E q)" where "$(page -E q)"
    /// evaluates not into /path/to/sink as expected but into neovim UI instead. It consists of
    /// a bunch of characters and strings, so many useless files may be created and even overwriting
    /// of existed files might occur if their name would match. To prevent that, a path to dumb directory
    /// is printed first before neovim process was spawned. This expands to "cli > dir {neovim UI}"
    /// command which fails early as redirecting text into directory is impossible.
    fn print_redirect_protection(tmp_dir: &PathBuf) {
        let d = tmp_dir.clone().join("DO-NOT-REDIRECT-OUTSIDE-OF-NVIM-TERM(--help[-W])");
        if let Err(e) = std::fs::create_dir_all(&d) {
            panic!("Cannot create protection directory '{}': {:?}", d.display(), e)
        }
        println!("{}", d.to_string_lossy());
    }

    /// Spawns child neovim process and connects to it using socket.
    /// This not uses neovim's "--embed" flag, so neovim UI is displayed properly on top of page.
    /// Also this neovim child process doesn't inherits page stdin, therefore
    /// page is able to operate on its input and redirect it into a proper target.
    /// Also custom neovim config would be used if it's present in corresponding location.
    fn spawn_child_nvim_process(opt: &Options, nvim_listen_addr: &str) -> process::Child {
        let nvim_args = {
            let mut a = String::new();
            a.push_str("--cmd 'set shortmess+=I' ");
            a.push_str("--listen ");
            a.push_str(nvim_listen_addr);
            if let Some(config) = opt.config.clone().or_else(default_config_path) {
                a.push(' ');
                a.push_str("-u ");
                a.push_str(&config);
            }
            if let Some(custom_args) = opt.arguments.as_ref() {
                a.push(' ');
                a.push_str(custom_args);
            }
            shell_words::split(&a).expect("Cannot parse neovim arguments")
        };
        log::trace!(target: "New neovim process", "args: {:?}", nvim_args);
        process::Command::new("nvim").args(&nvim_args)
            .stdin(process::Stdio::null())
            .spawn()
            .expect("Cannot spawn a child neovim process")
    }

    /// Returns path to custom neovim config if it's present in corresponding locations.
    fn default_config_path() -> Option<String> {
        std::env::var("XDG_CONFIG_HOME").ok().and_then(|xdg_config_home| {
            let p = PathBuf::from(xdg_config_home).join("page/init.vim");
            if p.exists() {
                log::trace!(target: "default config", "use $XDG_CONFIG_HOME: {}", p.display());
                Some(p)
            } else {
                None
            }
        })
        .or_else(|| std::env::var("HOME").ok().and_then(|home_dir| {
            let p = PathBuf::from(home_dir).join(".config/page/init.vim");
            if p.exists() {
                log::trace!(target: "default config", "use ~/.config: {}", p.display());
                Some(p)
            } else {
                None
            }
        }))
        .map(|p| p.to_string_lossy().to_string())
    }

    /// Returns neovim session either backed by TCP or UNIX socket
    fn session_at_address(nvim_listen_addr: &str) -> std::io::Result<neovim_lib::Session> {
        let session = match nvim_listen_addr.parse::<std::net::SocketAddr>() {
            Ok (_) => neovim_lib::Session::new_tcp(nvim_listen_addr)?,
            Err(_) => neovim_lib::Session::new_unix_socket(nvim_listen_addr)?,
        };
        Ok(session)
    }
}
