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
      "i Run `rok::setup()` to install it.",
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
      "i Run `rok::setup()` to install it again.",
      call. = FALSE
    )
  }
  if (!identical(found, wanted)) {
    # A newer binary (after `rok self update`) asks for a newer package, not an older binary.
    fix <- if (utils::compareVersion(found, wanted) > 0) {
      paste0("i Update the rok package to ", found, ", or run `rok::setup()` to go back to rok ", wanted, ".")
    } else {
      "i Run `rok::setup()` to install the matching binary."
    }
    stop("The rok binary is version ", found, ", but the R package is ", wanted, ".\n", fix,
      call. = FALSE
    )
  }
  invisible(TRUE)
}

# Runs the binary with `args` and `--json`. Messages for people go straight to the console
# (stderr); the JSON result on stdout is parsed and returned. When the binary needs an
# answer (exit status 2 with a `needs` object), the question is asked here and the command
# runs again with the answer.
rok_call <- function(args) {
  bin <- rok_binary()
  answers <- character()
  repeat {
    out <- tempfile("rok-")
    # system2() quotes the command but passes the arguments to the shell as they are: quote
    # them (`< 0.13`, paths with spaces).
    status <- system2(bin, shQuote(c(args, "--json", answers)), stdout = out, stderr = "")
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

# Asks a question the binary handed back, and returns the arguments that answer it.
ask <- function(needs) {
  yes <- isTRUE(getOption("rok.yes"))
  if (!yes && !interactive()) {
    stop(needs$question, "\n",
      "i This needs an answer. Run it in an interactive session, ",
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
  answer <- if (yes) TRUE else utils::askYesNo(needs$question, default = isTRUE(needs$default))
  if (!isTRUE(answer)) stop("Cancelled. Nothing was changed.", call. = FALSE)
  c("--confirmed", needs$id)
}

# Arguments for an optional value: `--flag value`, or nothing.
opt <- function(flag, value) {
  if (is.null(value)) character() else c(flag, as.character(value))
}
