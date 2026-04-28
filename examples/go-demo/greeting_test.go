package greeting

import "testing"

func TestGreet(t *testing.T) {
	t.Parallel()

	if got := Greet("Ada"); got != "Hello Ada from Go localci" {
		t.Fatalf("unexpected greeting: %s", got)
	}

	if got := Greet(""); got != "Hello developer from Go localci" {
		t.Fatalf("unexpected default greeting: %s", got)
	}
}
