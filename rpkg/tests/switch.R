# What init() does afterwards (no IDE here, so only the decision is tested).
after_init_action <- rok:::after_init_action
stopifnot(
  identical(after_init_action(TRUE, "positron", character(), TRUE), "restart"),
  identical(after_init_action(FALSE, "rstudio", "dplyr", TRUE), "open"),
  identical(after_init_action(TRUE, "none", character(), TRUE), "activate"),
  identical(after_init_action(FALSE, "none", character(), TRUE), "activate"),
  identical(after_init_action(TRUE, "none", "dplyr", TRUE), "advise"),
  identical(after_init_action(TRUE, "positron", character(), FALSE), "switch-r"),
  identical(after_init_action(TRUE, "none", character(), FALSE), "switch-r")
)
