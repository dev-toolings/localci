package greeting

import "strings"

func Greet(name string) string {
	cleanName := strings.TrimSpace(name)
	if cleanName == "" {
		cleanName = "developer"
	}

	return "Hello " + cleanName + " from Go localci"
}
