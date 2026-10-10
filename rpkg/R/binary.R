# Finding the rok binary and calling it (requirements chapter 4: the package is a thin front
# end; the binary does the work and builds the messages).

# Where rok keeps its data: tools::R_user_dir("rok", "data"), as the binary does.
data_dir <- function() {
  dir <- Sys.getenv("ROK_DATA_DIR")
  if (nzchar(dir)) dir else tools::R_user_dir("rok", "data")
}

exe <- function(name) {
  if (.Platform$OS.type == "windows") paste0(name, ".exe") else name
}

# Where setup() puts the binary.
binary_path <- function() {
  file.path(data_dir(), "bin", exe("rok"))
}

the <- new.env(parent = emptyenv())

#' Locate the rok binary
#'
#' Looks for the binary in this order: the option `rok.binary`, the environment variable
#' `ROK_BINARY`, the place [setup()] puts it, and the `PATH`. The binary must have the same
#' version as this package.
#'
#' @returns The path of the binary. An error if no matching binary is found.
#' @seealso [rok_available()] to check without an error.
#' @export
#' @examplesIf rok_available()
#' rok_binary()
rok_binary <- function() {
  if (!is.null(the$binary) && file.exists(the$binary)) {
    return(the$binary)
  }
  candidates <- c(getOption("rok.binary", ""), Sys.getenv("ROK_BINARY"), binary_path())
  candidates <- candidates[nzchar(candidates) & file.exists(candidates)]
  # Searching PATH runs `which`, which is slow: only when nothing else is found.
  if (!length(candidates) && nzchar(on_path <- Sys.which("rok"))) candidates <- on_path
  if (!length(candidates)) {
    stop(
      "The rok binary is not installed.\n",
      "\u2139 Run `rok::setup()` to install it.",
      call. = FALSE
    )
  }
  bin <- normalizePath(candidates[[1L]])
  check_version(bin)
  the$binary <- bin
  bin
}

#' Whether the rok binary is ready
#'
#' Checks that a rok binary with the same version as this package can be found (see
#' [rok_binary()]). If not, [setup()] installs one.
#'
#' @returns `TRUE` or `FALSE`.
#' @export
#' @examples
#' rok_available()
rok_available <- function() {
  !inherits(tryCatch(rok_binary(), error = identity), "error")
}

# The version a binary reports (`rok 0.1.0`), or NA if it does not run.
binary_version <- function(bin) {
  out <- tryCatch(
    suppressWarnings(system2(bin, "--version", stdout = TRUE, stderr = FALSE)),
    error = function(e) character()
  )
  if (!length(out) || !grepl("^rok ", out[[1L]])) return(NA_character_)
  sub("^rok ", "", trimws(out[[1L]]))
}

check_version <- function(bin) {
  found <- binary_version(bin)
  wanted <- as.character(utils::packageVersion("rok"))
  if (is.na(found)) {
    stop("`", bin, "` is not a working rok binary.\n",
      "\u2139 Run `rok::setup()` to install it again.",
      call. = FALSE
    )
  }
  if (!identical(found, wanted)) {
    # A newer binary (after `rok self update`) asks for a newer package, not an older binary.
    fix <- if (utils::compareVersion(found, wanted) > 0) {
      paste0("\u2139 Update the rok package to ", found, ", or run `rok::setup()` to go back to rok ", wanted, ".")
    } else {
      "\u2139 Run `rok::setup()` to install the matching binary."
    }
    stop("The rok binary is version ", found, ", but the R package is ", wanted, ".\n", fix,
      call. = FALSE
    )
  }
  invisible(TRUE)
}

# Runs the binary with `args` and `--json`. Messages for people go to the console (stderr);
# the JSON result on stdout is parsed and returned. When the binary needs an answer (exit
# status 2 with a `needs` object), the question is asked here and the command runs again with
# the answer. In an interactive session the binary runs in the background, so that its
# progress can be drawn (see run_watched()).
rok_call <- function(args) {
  bin <- rok_binary()
  answers <- character()
  repeat {
    out <- tempfile("rok-")
    status <- if (interactive()) {
      run_watched(bin, c(args, "--json", answers), out)
    } else {
      # system2() quotes the command but passes the arguments to the shell as they are: quote
      # them (`< 0.13`, paths with spaces).
      system2(bin, shQuote(c(args, "--json", answers)), stdout = out, stderr = "")
    }
    text <- if (file.exists(out)) readLines(out, warn = FALSE, encoding = "UTF-8") else character()
    unlink(out)
    result <- if (length(text)) parse_json(paste(text, collapse = "\n")) else NULL
    if (identical(as.integer(status), 2L) && !is.null(result$needs)) {
      answers <- c(answers, ask(result$needs))
      next
    }
    if (!identical(as.integer(status), 0L)) {
      stop("rok could not complete the command (see the messages above).", call. = FALSE)
    }
    return(result)
  }
}

# Runs the binary in the background with `--events <file>`, writing stdout to `out`, and
# relays what it writes to that file: messages as they come, and the progress line, drawn here
# with `\r`. (Waiting in system2() would hold the progress back: in RStudio and Positron on
# Windows, R shows a program's output only line by line. And on Windows, R cannot read a file
# that system2() redirects output to until the program exits, so the binary writes the events
# file itself.) Returns the exit status. If R is interrupted, the binary is stopped too.
run_watched <- function(bin, args, out) {
  events <- tempfile("rok-")
  err <- tempfile("rok-")
  file.create(events)
  con <- NULL
  pid <- NA_integer_
  status <- NA_integer_
  ended <- FALSE
  shown <- 0L
  on.exit({
    if (shown > 0L) message("\r", strrep(" ", shown), "\r", appendLF = FALSE)
    if (!ended && !is.na(pid)) tools::pskill(pid)
    if (!is.null(con)) close(con)
    unlink(c(events, err))
  })
  system2(bin, shQuote(c(args, "--events", events)), stdout = out, stderr = err, wait = FALSE)
  con <- file(events, open = "r", blocking = FALSE, encoding = "UTF-8")
  started <- checked <- Sys.time()
  repeat {
    lines <- readLines(con, warn = FALSE)
    for (line in lines) {
      if (!startsWith(line, "\001")) {
        if (shown > 0L) {
          message("\r", strrep(" ", shown), "\r", appendLF = FALSE)
          shown <- 0L
        }
        message(line)
        next
      }
      kind <- sub("^\001(\\S+).*$", "\\1", line)
      value <- sub("^\001\\S+ ?", "", line)
      if (kind == "pid") {
        pid <- as.integer(value)
      } else if (kind == "exit") {
        status <- as.integer(value)
      } else if (kind == "progress") {
        # An empty line clears the progress.
        message("\r", value, strrep(" ", max(0L, shown - nchar(value))),
          if (!nzchar(value)) "\r", appendLF = FALSE)
        shown <- nchar(value)
      }
    }
    if (!is.na(status)) break
    if (ended) {
      # The binary stopped without saying how (it crashed or was killed, or never started):
      # show what it wrote to stderr.
      for (line in readable_lines(err)) message(line)
      status <- 1L
      break
    }
    if (!length(lines)) {
      if (is.na(pid) && difftime(Sys.time(), started, units = "secs") > 10) {
        ended <- TRUE
        next
      }
      if (!is.na(pid) && difftime(Sys.time(), checked, units = "secs") > 1) {
        checked <- Sys.time()
        # Read once more: the last lines may have come just before it stopped.
        if (!process_alive(pid)) ended <- TRUE
        next
      }
      Sys.sleep(0.05)
    }
  }
  ended <- TRUE
  # The binary reported its exit status just before exiting; on Windows, its output can be
  # read only once it has.
  for (i in 1:250) {
    if (readable(out)) break
    Sys.sleep(0.02)
  }
  status
}

# Whether the file `path` can be opened for reading.
readable <- function(path) {
  con <- suppressWarnings(tryCatch(file(path, open = "r"), error = function(e) NULL))
  if (is.null(con)) return(FALSE)
  close(con)
  TRUE
}

# The lines of `path`, or none if it cannot be read.
readable_lines <- function(path) {
  if (!readable(path)) return(character())
  readLines(path, warn = FALSE, encoding = "UTF-8")
}

# Whether the process `pid` is running.
process_alive <- function(pid) {
  if (.Platform$OS.type == "windows") {
    out <- suppressWarnings(system2("tasklist",
      c("/FI", shQuote(paste("PID eq", pid)), "/NH", "/FO", "CSV"),
      stdout = TRUE, stderr = FALSE
    ))
    # tasklist answers in the system's code page: compare bytes (the pid is ASCII).
    any(grepl(paste0('","', pid, '","'), out, fixed = TRUE, useBytes = TRUE))
  } else {
    tools::pskill(pid, 0L)
  }
}

# Asks a question the binary handed back, and returns the arguments that answer it.
ask <- function(needs) {
  yes <- isTRUE(getOption("rok.yes"))
  if (!yes && !interactive()) {
    stop(needs$question, "\n",
      "\u2139 This needs an answer. Run it in an interactive session, ",
      "or set `options(rok.yes = TRUE)` to answer yes to every question.",
      call. = FALSE
    )
  }
  if (!is.null(needs$options)) {
    labels <- vapply(needs$options, function(o) o$label, character(1))
    values <- vapply(needs$options, function(o) o$value, character(1))
    pick <- if (yes) 1L else utils::menu(labels, title = needs$question)
    if (pick == 0L) stop("Cancelled. Nothing was changed.", call. = FALSE)
    return(c(needs$flag, values[[pick]]))
  }
  answer <- if (yes) TRUE else ask_yes_no(needs$question, default = isTRUE(needs$default))
  if (!isTRUE(answer)) stop("Cancelled. Nothing was changed.", call. = FALSE)
  c("--confirmed", needs$id)
}

# askYesNo() in the console: TRUE, FALSE or NA (cancelled). Giving the prompts keeps it from
# using the askYesNo option, which R sets to a Windows dialog when it starts as Rgui. Positron
# starts R that way on Windows, and the dialog opens behind its window: R seems to hang.
ask_yes_no <- function(question, default = TRUE) {
  utils::askYesNo(question, default = default, prompts = c("Yes", "No", "Cancel"))
}

# Arguments for an optional value: `--flag value`, or nothing.
opt <- function(flag, value) {
  if (is.null(value)) character() else c(flag, as.character(value))
}
