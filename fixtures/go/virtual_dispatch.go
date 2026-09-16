package go_sample

// Interface dispatch with two implementors — resolves over-approximate.
type Barker interface {
	Bark() string
}

type Dog struct{}
type Cat struct{}

func (d *Dog) Bark() string { return "woof" }
func (c *Cat) Bark() string { return "meow" }

func MakeBark(b Barker) string { return b.Bark() }
