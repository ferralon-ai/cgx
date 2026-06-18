// cgx-fixture: closures and iterators
// Covers: calls:closure (closure invocation), calls:callback (fn-value arg),
//         implicit:iterator (for loop over iterator), closure captures

/// Closure assigned to a variable and invoked — calls:closure.
pub fn closure_variable() -> i32 {
    let double = |x: i32| x * 2;
    double(5)
}

/// Iterator body — implicit:iterator on the for loop's `next()` calls.
pub fn sum_vec(items: &[i32]) -> i32 {
    let mut total = 0;
    for item in items.iter() {
        total += item;
    }
    total
}

/// .map() with a closure — calls:closure on the map body.
pub fn double_all(items: &[i32]) -> Vec<i32> {
    items.iter().map(|x| x * 2).collect()
}

/// Closure that captures a variable from outer scope.
pub fn make_adder(n: i32) -> impl Fn(i32) -> i32 {
    move |x| x + n
}

pub fn use_captured_closure() -> i32 {
    let add5 = make_adder(5);
    add5(10)
}

/// Higher-order function — calls:callback on invocation of `f`.
pub fn apply<F: Fn(i32) -> i32>(f: F, value: i32) -> i32 {
    f(value)
}

pub fn use_callback() -> i32 {
    apply(|x| x + 1, 41)
}

/// Closure passed as a function pointer — calls:callback.
pub fn transform_list(items: &mut Vec<i32>, f: fn(i32) -> i32) {
    for item in items.iter_mut() {
        *item = f(*item);
    }
}

fn negate(x: i32) -> i32 {
    -x
}

pub fn negate_list(items: &mut Vec<i32>) {
    transform_list(items, negate);
}

/// Nested closures — closure capturing another closure.
pub fn nested_closures() -> i32 {
    let outer = |x: i32| {
        let inner = |y: i32| y * y;
        inner(x) + x
    };
    outer(3)
}

/// .filter() + .map() chain — multiple closure invocations.
pub fn filter_and_double(items: &[i32], threshold: i32) -> Vec<i32> {
    items.iter()
        .filter(|&&x| x > threshold)
        .map(|&x| x * 2)
        .collect()
}
