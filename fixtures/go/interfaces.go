package go_sample

type Reader interface {
	Read() int
}

type File struct{ n int }

func (f *File) Read() int { return f.n }

func Consume(r Reader) int { return r.Read() }
