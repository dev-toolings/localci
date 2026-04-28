package main

import (
	"fmt"
	"os"

	greeting "example.com/localci-go-demo"
)

func main() {
	name := "developer"
	if len(os.Args) > 1 {
		name = os.Args[1]
	}

	fmt.Println(greeting.Greet(name))
}
