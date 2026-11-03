include "root" {
  path = find_in_parent_folders("root.hcl")
}

inputs = {
  name = "app"
  port = 8080
}
